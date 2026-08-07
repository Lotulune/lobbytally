#!/bin/sh
set -eu

if [ "${MPGS_UPDATE_REEXEC:-0}" != 1 ]; then
  original_script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
  update_copy=$(mktemp "${TMPDIR:-/tmp}/mpgs-update.XXXXXX")
  cp -- "$0" "$update_copy"
  chmod 0700 "$update_copy"
  export MPGS_UPDATE_REEXEC=1
  export MPGS_UPDATE_SCRIPT_DIR="$original_script_dir"
  export MPGS_UPDATE_TEMP_SCRIPT="$update_copy"
  exec "$update_copy" "$@"
fi

script_dir=${MPGS_UPDATE_SCRIPT_DIR:?missing original update script directory}
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
env_file="$script_dir/.env"
source_compose_file="$script_dir/docker-compose.yml"
runtime_dir="$script_dir/runtime"
old_compose_file=
release_compose_file=
new_compose_file=
rollback_armed=0

cleanup_temp_files() {
  if [ -n "${release_compose_file:-}" ]; then
    rm -f -- "$release_compose_file"
  fi
  if [ -n "${old_compose_file:-}" ]; then
    rm -f -- "$old_compose_file"
  fi
  if [ -n "${MPGS_UPDATE_TEMP_SCRIPT:-}" ]; then
    rm -f -- "$MPGS_UPDATE_TEMP_SCRIPT"
  fi
}

handle_signal() {
  trap - HUP INT TERM
  if [ "${rollback_armed:-0}" -eq 1 ]; then
    rollback_armed=0
    if command -v rollback >/dev/null 2>&1; then
      rollback || true
    elif command -v restart_previous_release >/dev/null 2>&1; then
      restart_previous_release || true
    fi
  fi
  exit 130
}

trap cleanup_temp_files EXIT
trap handle_signal HUP INT TERM

if [ ! -f "$env_file" ]; then
  printf 'Missing %s; copy deploy/.env.example and review it first.\n' "$env_file" >&2
  exit 2
fi

set -a
# deploy/.env is administrator-owned and must contain shell-compatible KEY=value lines.
. "$env_file"
set +a

branch=${MPGS_DEPLOY_BRANCH:-main}
mode=${MPGS_DEPLOY_MODE:-full}
backup_retention_count=${MPGS_BACKUP_RETENTION_COUNT:-3}
health_timeout_secs=${MPGS_DEPLOY_HEALTH_TIMEOUT_SECS:-600}

require_bounded_positive_integer() {
  name=$1
  value=$2
  maximum=$3
  case "$value" in
    ''|*[!0-9]*|0)
      printf '%s must be a positive integer (got: %s)\n' "$name" "$value" >&2
      exit 2
      ;;
  esac
  if [ "$value" -gt "$maximum" ]; then
    printf '%s must not exceed %s (got: %s)\n' "$name" "$maximum" "$value" >&2
    exit 2
  fi
}

require_bounded_positive_integer MPGS_BACKUP_RETENTION_COUNT \
  "$backup_retention_count" 100
require_bounded_positive_integer MPGS_DEPLOY_HEALTH_TIMEOUT_SECS \
  "$health_timeout_secs" 3600

case "$mode" in
  backend)
    services="mpgs-server mpgs-worker"
    # Include the Web service so switching from full to backend cannot leave the
    # previous frontend running.
    stop_services="mpgs-web mpgs-worker mpgs-server"
    health_port=${MPGS_API_PORT:-18081}
    ;;
  full)
    services="mpgs-server mpgs-worker mpgs-web"
    stop_services="mpgs-web mpgs-worker mpgs-server"
    health_port=18082
    ;;
  *)
    printf 'MPGS_DEPLOY_MODE must be backend or full (got: %s)\n' "$mode" >&2
    exit 2
    ;;
esac

old_compose_file=$(mktemp "${TMPDIR:-/tmp}/mpgs-compose-rollback.XXXXXX")
cp -- "$source_compose_file" "$old_compose_file"
new_compose_file="$old_compose_file"
compose_project_name=${COMPOSE_PROJECT_NAME:-$(basename "$script_dir")}

old_compose() {
  docker compose \
    --project-name "$compose_project_name" \
    --project-directory "$script_dir" \
    --env-file "$env_file" \
    -f "$old_compose_file" "$@"
}

new_compose() {
  docker compose \
    --project-name "$compose_project_name" \
    --project-directory "$script_dir" \
    --env-file "$env_file" \
    -f "$new_compose_file" "$@"
}

repository_from_image() {
  image_ref=$1
  case "$image_ref" in
    *@*)
      printf '%s\n' "${image_ref%@*}"
      ;;
    *)
      image_name=${image_ref##*/}
      case "$image_name" in
        *:*) printf '%s\n' "${image_ref%:*}" ;;
        *) printf '%s\n' "$image_ref" ;;
      esac
      ;;
  esac
}

validate_release_sha() {
  candidate_sha=$1
  case "$candidate_sha" in
    ''|*[!0-9a-fA-F]*)
      printf 'Release SHA must be a 40-character hexadecimal Git SHA (got: %s)\n' \
        "$candidate_sha" >&2
      return 1
      ;;
  esac
  if [ "${#candidate_sha}" -ne 40 ]; then
    printf 'Release SHA must contain 40 hexadecimal characters (got length %s).\n' \
      "${#candidate_sha}" >&2
    return 1
  fi
}

server_repository=${MPGS_SERVER_REPOSITORY:-}
if [ -z "$server_repository" ] && [ -n "${MPGS_SERVER_IMAGE:-}" ]; then
  server_repository=$(repository_from_image "$MPGS_SERVER_IMAGE")
fi
web_repository=${MPGS_WEB_REPOSITORY:-}
if [ -z "$web_repository" ] && [ -n "${MPGS_WEB_IMAGE:-}" ]; then
  web_repository=$(repository_from_image "$MPGS_WEB_IMAGE")
fi
if [ -z "$server_repository" ] || { [ "$mode" = "full" ] && [ -z "$web_repository" ]; }; then
  printf 'Set MPGS_SERVER_REPOSITORY and MPGS_WEB_REPOSITORY in %s.\n' "$env_file" >&2
  exit 2
fi

release_sha=${MPGS_RELEASE_SHA:-}
follow_pointer=1
if [ -n "$release_sha" ]; then
  follow_pointer=0
  validate_release_sha "$release_sha"
else
  release_pointer=${MPGS_RELEASE_POINTER_IMAGE:-"${server_repository}:release-main"}
  docker pull "$release_pointer" >/dev/null
  release_sha=$(
    docker image inspect \
      --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' \
      "$release_pointer"
  )
  validate_release_sha "$release_sha"
fi
release_sha=$(printf '%s' "$release_sha" | tr '[:upper:]' '[:lower:]')

cd "$repo_root"
advance_source=0
old_source_sha=
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if [ "$follow_pointer" -eq 1 ]; then
    current_branch=$(git branch --show-current)
    if [ "$current_branch" != "$branch" ]; then
      printf 'Expected deployment branch %s, but checkout is on %s; refusing to switch it.\n' \
        "$branch" "${current_branch:-detached HEAD}" >&2
      exit 1
    fi
    if [ -n "$(git status --porcelain --untracked-files=normal)" ]; then
      printf 'Deployment checkout is dirty; refusing an automatic source transition.\n' >&2
      exit 1
    fi
    old_source_sha=$(git rev-parse HEAD)
    git fetch origin "$branch"
    fetched_sha=$(git rev-parse FETCH_HEAD)
    if [ "$fetched_sha" != "$release_sha" ]; then
      printf \
        'Validated release %s is not the tip of origin/%s (%s); keeping the current deployment.\n' \
        "$release_sha" "$branch" "$fetched_sha"
      exit 0
    fi
    release_compose_file=$(mktemp "${TMPDIR:-/tmp}/mpgs-compose-release.XXXXXX")
    git show "${release_sha}:deploy/docker-compose.yml" >"$release_compose_file"
    new_compose_file="$release_compose_file"
    advance_source=1
  else
    printf 'MPGS_RELEASE_SHA is pinned; leaving the source checkout unchanged.\n'
  fi
else
  printf 'Source directory is not a Git checkout; using the packaged deployment files.\n'
fi

new_server_image="${server_repository}:sha-${release_sha}"
new_web_image=
docker pull "$new_server_image" >/dev/null
if [ "$mode" = "full" ]; then
  new_web_image="${web_repository}:sha-${release_sha}"
  docker pull "$new_web_image" >/dev/null
fi

old_server_container=$(old_compose ps -q mpgs-server 2>/dev/null || true)
old_worker_container=$(old_compose ps -q mpgs-worker 2>/dev/null || true)
old_web_container=$(old_compose ps -q mpgs-web 2>/dev/null || true)
any_web_container=$(old_compose ps -q --all mpgs-web 2>/dev/null || true)
old_server_image_id=
old_worker_image_id=
old_web_image_id=
old_server_image=
old_web_image=
old_release_sha=
old_web_release_sha=
if [ -n "$old_server_container" ]; then
  old_server_image_id=$(docker inspect --format '{{.Image}}' "$old_server_container")
  old_release_sha=$(
    docker inspect \
      --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' \
      "$old_server_container" 2>/dev/null || true
  )
fi
if [ -n "$old_worker_container" ]; then
  old_worker_image_id=$(docker inspect --format '{{.Image}}' "$old_worker_container")
fi
if [ -n "$old_web_container" ]; then
  old_web_image_id=$(docker inspect --format '{{.Image}}' "$old_web_container")
  old_web_release_sha=$(
    docker inspect \
      --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' \
      "$old_web_container" 2>/dev/null || true
  )
fi

deployment_healthcheck() {
  compose_runner=$1
  meta=
  curl --fail --silent --show-error \
      "http://127.0.0.1:${health_port}/health/ready" >/dev/null 2>&1 \
    || return 1
  meta=$(curl --fail --silent --show-error \
    "http://127.0.0.1:${health_port}/v1/meta" 2>/dev/null) \
    || return 1
  if ! printf '%s' "$meta" \
    | grep -F "\"build_git_sha\":\"$release_sha\"" >/dev/null; then
    # The old service set has already been stopped before the replacement is
    # started, so a responsive endpoint with another immutable revision cannot
    # become the requested release by waiting. Distinguish this fatal mismatch
    # from temporary startup/worker-health failures.
    return 2
  fi
  "$compose_runner" exec -T mpgs-worker \
    /usr/local/bin/mpgs-worker-loop --healthcheck >/dev/null 2>&1
}

prune_pre_update_backups() {
  target_container=$1
  docker exec "$target_container" /bin/sh -c '
    set -eu
    keep=$1
    first_stale=$((keep + 1))
    stale=$(
      find /var/lib/mpgs/backups -maxdepth 1 -type f \
        -name "pre-update-*.db" -printf "%T@ %p\n" \
        | sort -nr \
        | tail -n "+$first_stale" \
        | cut -d " " -f 2-
    )
    removed=0
    for path in $stale; do
      case "$path" in
        /var/lib/mpgs/backups/pre-update-*.db)
          rm -f -- "$path"
          removed=$((removed + 1))
          ;;
        *)
          printf "Refusing to prune unexpected backup path: %s\n" "$path" >&2
          exit 1
          ;;
      esac
    done
    printf "%s\n" "$removed"
  ' sh "$backup_retention_count"
}

advance_source_checkout() {
  [ "$advance_source" -eq 1 ] || return 0
  # A fast-forward is brief but must not be interrupted between worktree and
  # ref updates; other deployment phases remain signal-aware and roll back.
  trap '' HUP INT TERM
  if git merge --ff-only "$release_sha" \
    && [ "$(git rev-parse HEAD)" = "$release_sha" ]; then
    transition_result=0
    # The source and validated deployment now point at the same immutable SHA;
    # a later signal must not roll containers back behind the checkout.
    rollback_armed=0
  else
    transition_result=1
  fi
  trap handle_signal HUP INT TERM
  return "$transition_result"
}

old_release_sha=$(printf '%s' "$old_release_sha" | tr '[:upper:]' '[:lower:]')
old_web_release_sha=$(printf '%s' "$old_web_release_sha" | tr '[:upper:]' '[:lower:]')
mode_matches=0
case "$mode" in
  backend)
    if [ -n "$old_server_container" ] \
      && [ -n "$old_worker_container" ] \
      && [ -z "$any_web_container" ]; then
      mode_matches=1
    fi
    ;;
  full)
    if [ -n "$old_server_container" ] \
      && [ -n "$old_worker_container" ] \
      && [ -n "$old_web_container" ] \
      && [ "$old_web_release_sha" = "$release_sha" ]; then
      mode_matches=1
    fi
    ;;
esac

if [ "$mode_matches" -eq 1 ] \
  && [ "$old_release_sha" = "$release_sha" ] \
  && [ "$old_worker_image_id" = "$old_server_image_id" ] \
  && { [ -z "$old_source_sha" ] || [ "$old_source_sha" = "$release_sha" ]; } \
  && deployment_healthcheck old_compose; then
  if ! advance_source_checkout; then
    printf 'Healthy containers are at %s, but the source fast-forward failed.\n' \
      "$release_sha" >&2
    exit 1
  fi
  if pruned_count=$(prune_pre_update_backups "$old_server_container"); then
    printf 'Pruned %s old pre-update backup(s); retaining the newest %s.\n' \
      "$pruned_count" "$backup_retention_count"
  else
    printf 'Warning: could not prune old pre-update backups.\n' >&2
  fi
  printf 'MPGS %s deployment is already healthy at %s\n' "$mode" "$release_sha"
  exit 0
fi

timestamp=$(date -u +%Y%m%dT%H%M%SZ)
if [ -n "$old_server_container" ]; then
  if [ -n "$old_worker_container" ] \
    && [ "$old_worker_image_id" != "$old_server_image_id" ]; then
    printf 'Server and worker use different old images; refusing an inexact automatic rollback.\n' >&2
    exit 1
  fi
  # Docker reports a container's exact image as an image ID. Tag that ID with
  # a normal local reference because Compose image fields are image references,
  # and a mutable original tag may no longer point at the running image.
  old_server_image="mpgs-rollback-server:${timestamp}-$$"
  docker image tag "$old_server_image_id" "$old_server_image"
fi
if [ -n "$old_web_container" ]; then
  old_web_image="mpgs-rollback-web:${timestamp}-$$"
  docker image tag "$old_web_image_id" "$old_web_image"
fi

old_services="mpgs-server"
if [ -n "$old_worker_container" ]; then
  old_services="$old_services mpgs-worker"
fi
if [ -n "$old_web_container" ]; then
  old_services="$old_services mpgs-web"
fi

restart_previous_release() {
  if [ -z "$old_server_image" ]; then
    return 1
  fi
  export MPGS_SERVER_IMAGE="$old_server_image"
  if [ -n "$old_web_image" ]; then
    export MPGS_WEB_IMAGE="$old_web_image"
  fi
  old_compose up -d --no-build --pull never --remove-orphans $old_services
}

backup_rel=
backup_created=0

# The init container deliberately makes the bind-mounted runtime private to the
# in-container mpgs user. The host deployment user may therefore be unable to
# traverse the directory, and a host-side `[ -f ]` would silently report that a
# real database is absent. Probe through the already-validated server image so
# permission checks use the same mount namespace as backup and restore.
if ! runtime_db_state=$(
  docker run --rm --user 0:0 --network none --read-only \
    --entrypoint /bin/sh \
    --mount "type=bind,src=$runtime_dir,dst=/var/lib/mpgs,readonly" \
    "$new_server_image" \
    -c 'if [ -f /var/lib/mpgs/mpgs.db ]; then printf present; else printf absent; fi'
); then
  printf 'Could not inspect the runtime database through the release image; refusing the upgrade.\n' >&2
  exit 1
fi
case "$runtime_db_state" in
  present|absent) ;;
  *)
    printf 'Runtime database probe returned an invalid state; refusing the upgrade.\n' >&2
    exit 1
    ;;
esac

if [ -n "$old_server_container" ]; then
  # Quiesce every writer before the backup so rollback cannot lose requests
  # accepted between an online backup and container replacement.
  rollback_armed=1
  if ! old_compose stop $stop_services; then
    rollback_armed=0
    printf 'Could not stop the current deployment cleanly; restoring its service set.\n' >&2
    restart_previous_release || true
    exit 1
  fi
  if [ "$runtime_db_state" = present ] && [ -n "$old_worker_container" ]; then
    # The stopped worker may have held a 30-minute job lease. With every
    # application writer quiesced, returning those Steam jobs to pending is
    # safe and prevents each deployment from creating an ingestion gap.
    if ! docker run --rm --network none --read-only \
      --entrypoint /usr/local/bin/mpgs-dbtool \
      --mount "type=bind,src=$runtime_dir,dst=/var/lib/mpgs" \
      "$new_server_image" \
      recover-steam-leases /var/lib/mpgs/mpgs.db; then
      printf 'Could not recover stopped worker leases; restarting the previous release.\n' >&2
      restart_previous_release || true
      exit 1
    fi
  fi
  if [ "$runtime_db_state" = present ]; then
    old_short=unknown
    if validate_release_sha "$old_release_sha" >/dev/null 2>&1; then
      old_short=$(printf '%s' "$old_release_sha" | cut -c1-12)
    fi
    backup_rel="backups/pre-update-${timestamp}-${old_short}-$$.db"
    if ! docker run --rm --user 0:0 \
      --entrypoint /bin/sh \
      --mount "type=bind,src=$runtime_dir,dst=/var/lib/mpgs" \
      "$old_server_image" \
      -c 'install -d -o mpgs -g mpgs -m 0750 /var/lib/mpgs/backups'; then
      printf 'Could not prepare the backup directory; restarting the previous release.\n' >&2
      restart_previous_release || true
      exit 1
    fi
    if ! docker run --rm \
      --entrypoint /usr/local/bin/mpgs-dbtool \
      --mount "type=bind,src=$runtime_dir,dst=/var/lib/mpgs" \
      "$old_server_image" \
      backup /var/lib/mpgs/mpgs.db "/var/lib/mpgs/$backup_rel"; then
      printf 'Pre-upgrade backup failed; restarting the previous release.\n' >&2
      restart_previous_release || true
      exit 1
    fi
    # `backup` verifies the temporary database before publishing the final file.
    backup_created=1
  fi
elif [ "$runtime_db_state" = present ]; then
  printf 'Database exists but mpgs-server is not running; refusing an unbacked upgrade.\n' >&2
  exit 1
fi

restore_pre_upgrade_backup() {
  [ "$backup_created" -eq 1 ] || return 0
  failed_rel="failed-upgrade-${timestamp}-${release_sha}-$$.db"
  docker run --rm --user 0:0 \
    --entrypoint /bin/sh \
    --mount "type=bind,src=$runtime_dir,dst=/var/lib/mpgs" \
    "$old_server_image" \
    -c '
      set -eu
      cd /var/lib/mpgs
      failed=$1
      backup=$2
      restore_tmp=".mpgs-rollback-$$.db"
      cp -- "$backup" "$restore_tmp"
      chown mpgs:mpgs "$restore_tmp"
      chmod 0640 "$restore_tmp"
      for current in mpgs.db mpgs.db-wal mpgs.db-shm; do
        if [ -e "$current" ]; then
          suffix=${current#mpgs.db}
          mv -- "$current" "${failed}${suffix}"
        fi
      done
      mv -- "$restore_tmp" mpgs.db
    ' sh "$failed_rel" "$backup_rel"
}

rollback() {
  rollback_armed=0
  printf 'Deployment validation failed; rolling back to the previous release.\n' >&2
  new_compose stop $stop_services >/dev/null 2>&1 || true
  if [ -z "$old_server_image" ]; then
    printf 'No previous release exists; failed containers remain stopped.\n' >&2
    return 1
  fi
  if ! restore_pre_upgrade_backup; then
    printf 'Automatic database restore failed; manual recovery is required.\n' >&2
    return 1
  fi
  if ! restart_previous_release; then
    printf 'Previous containers could not be restarted; manual recovery is required.\n' >&2
    return 1
  fi
  printf 'Previous release restored. Failed database copies remain under deploy/runtime.\n' >&2
  return 0
}

export MPGS_SERVER_IMAGE="$new_server_image"
if [ "$mode" = "full" ]; then
  export MPGS_WEB_IMAGE="$new_web_image"
elif ! old_compose rm -f mpgs-web >/dev/null; then
  printf 'Could not remove the stopped Web container while switching to backend mode.\n' >&2
  restart_previous_release || true
  exit 1
fi
export MPGS_BUILD_GIT_SHA="$release_sha"

if ! new_compose up -d --no-build --pull never --remove-orphans $services; then
  rollback || true
  exit 1
fi

validate_deployment() {
  max_attempts=$(( (health_timeout_secs + 1) / 2 ))
  attempt=1
  while [ "$attempt" -le "$max_attempts" ]; do
    # The quiesced pre-upgrade backup already passed the full integrity/FK
    # scan. Repeating that O(database size) check against the live database
    # here delayed every restart and duplicated deployment validation work.
    health_result=0
    deployment_healthcheck new_compose || health_result=$?
    case "$health_result" in
      0) return 0 ;;
      2)
        printf 'Deployment reports a build revision other than %s; refusing to retry it.\n' \
          "$release_sha" >&2
        return 1
        ;;
    esac
    sleep 2
    attempt=$((attempt + 1))
  done
  return 1
}

if ! validate_deployment; then
  rollback || true
  exit 1
fi

if [ "$advance_source" -eq 1 ]; then
  # Only advance the checkout after the exact target Compose configuration and
  # both immutable images have passed validation. The updater itself runs from
  # a temporary copy, so this cannot replace the executing script.
  if ! advance_source_checkout; then
    printf 'Source fast-forward failed after validation; rolling back the deployment.\n' >&2
    rollback || true
    exit 1
  fi
fi

rollback_armed=0
server_container=$(new_compose ps -q mpgs-server)
docker exec "$server_container" /bin/sh -c \
  'umask 077; printf "%s\n" "$1" > /var/lib/mpgs/.release-sha' \
  sh "$release_sha"

if pruned_count=$(prune_pre_update_backups "$server_container"); then
  printf 'Pruned %s old pre-update backup(s); retaining the newest %s.\n' \
    "$pruned_count" "$backup_retention_count"
else
  printf 'Warning: could not prune old pre-update backups.\n' >&2
fi

if [ -n "$old_server_image" ]; then
  docker image rm "$old_server_image" >/dev/null 2>&1 || true
fi
if [ -n "$old_web_image" ]; then
  docker image rm "$old_web_image" >/dev/null 2>&1 || true
fi

printf '\nMPGS %s deployment updated to %s\n' "$mode" "$release_sha"
if [ -n "$old_source_sha" ]; then
  printf 'Source fast-forward: %s -> %s\n' "$old_source_sha" "$release_sha"
fi
if [ "$backup_created" -eq 1 ]; then
  printf 'Pre-upgrade backup: deploy/runtime/%s\n' "$backup_rel"
fi
