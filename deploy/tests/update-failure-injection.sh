#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
update_script="$repo_root/deploy/update.sh"
health_helper="$repo_root/deploy/deployment-healthcheck-retry.sh"

new_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
old_sha=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb

fail() {
  printf 'update failure-injection test failed: %s\n' "$1" >&2
  exit 1
}

assert_event_contains_all() {
  file=$1
  marker=$2
  shift 2
  line=$(grep -F "$marker" "$file" | head -n 1)
  [ -n "$line" ] || fail "missing event: $marker"
  for needle in "$@"; do
    case "$line" in
      *"$needle"*) ;;
      *) fail "event '$marker' is missing '$needle': $line" ;;
    esac
  done
}

assert_contains() {
  file=$1
  needle=$2
  grep -F "$needle" "$file" >/dev/null || fail "missing event: $needle"
}

assert_not_contains() {
  file=$1
  needle=$2
  if grep -F "$needle" "$file" >/dev/null; then
    fail "unexpected event: $needle"
  fi
}

event_line() {
  file=$1
  needle=$2
  grep -n -F "$needle" "$file" | head -n 1 | cut -d ':' -f 1
}

assert_before() {
  file=$1
  first=$2
  second=$3
  first_line=$(event_line "$file" "$first")
  second_line=$(event_line "$file" "$second")
  case "$first_line:$second_line" in
    *[!0-9:]*|:*) fail "could not order events: $first -> $second" ;;
  esac
  [ "$first_line" -lt "$second_line" ] \
    || fail "event order violated: $first must precede $second"
}

write_fake_git() {
  path=$1
  cat >"$path" <<'EOF'
#!/bin/sh
set -eu
if [ "${1:-}" = rev-parse ] && [ "${2:-}" = --is-inside-work-tree ]; then
  printf 'true\n'
  exit 0
fi
printf 'unexpected fake git invocation: %s\n' "$*" >&2
exit 97
EOF
  chmod +x "$path"
}

write_fake_timeout() {
  path=$1
  cat >"$path" <<'EOF'
#!/bin/sh
set -eu
shift
exec "$@"
EOF
  chmod +x "$path"
}

write_fake_sleep() {
  path=$1
  cat >"$path" <<'EOF'
#!/bin/sh
exit 0
EOF
  chmod +x "$path"
}

write_fake_curl() {
  path=$1
  cat >"$path" <<'EOF'
#!/bin/sh
set -eu
printf 'curl:%s\n' "$*" >>"$FAKE_EVENTS"
if [ "$FAKE_SCENARIO" = health_timeout ]; then
  exit 22
fi
case "$*" in
  */v1/meta*) printf '{"build_git_sha":"%s"}\n' "$FAKE_NEW_SHA" ;;
esac
exit 0
EOF
  chmod +x "$path"
}

write_fake_docker() {
  path=$1
  cat >"$path" <<'EOF'
#!/bin/sh
set -eu

event() {
  printf '%s\n' "$1" >>"$FAKE_EVENTS"
}

last_arg() {
  last=
  for value in "$@"; do
    last=$value
  done
  printf '%s\n' "$last"
}

verb=${1:-}
case "$verb" in
  pull)
    event "pull:${2:-}"
    exit 0
    ;;
  inspect)
    args=$*
    container=$(last_arg "$@")
    case "$args" in
      *org.opencontainers.image.revision*)
        case "$container" in
          old-server|old-web) printf '%s\n' "$FAKE_OLD_SHA" ;;
          *) printf '\n' ;;
        esac
        ;;
      *'{{.Image}}'*)
        case "$container" in
          old-server|old-worker) printf 'sha256:old-server-image\n' ;;
          old-web) printf 'sha256:old-web-image\n' ;;
          *) printf 'sha256:unknown\n' ;;
        esac
        ;;
      *)
        printf 'unexpected docker inspect: %s\n' "$args" >&2
        exit 96
        ;;
    esac
    exit 0
    ;;
  image)
    event "image:${2:-}:${3:-}:${4:-}"
    exit 0
    ;;
  compose)
    shift
    compose_verb=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        ps|stop|up|exec|rm)
          compose_verb=$1
          shift
          break
          ;;
        *) shift ;;
      esac
    done
    case "$compose_verb" in
      ps)
        service=$(last_arg "$@")
        case "$service" in
          mpgs-server) printf 'old-server\n' ;;
          mpgs-worker) printf 'old-worker\n' ;;
          mpgs-web) printf 'old-web\n' ;;
          *) printf '\n' ;;
        esac
        ;;
      stop)
        event "compose_stop:${MPGS_SERVER_IMAGE:-unset}"
        ;;
      up)
        event "compose_up:server=${MPGS_SERVER_IMAGE:-unset}:web=${MPGS_WEB_IMAGE:-unset}:services=$*"
        case "${MPGS_SERVER_IMAGE:-}" in
          *":sha-$FAKE_NEW_SHA")
            if [ "$FAKE_SCENARIO" = new_up_failure ]; then
              exit 1
            fi
            ;;
        esac
        ;;
      exec)
        event "compose_exec:${MPGS_SERVER_IMAGE:-unset}"
        ;;
      rm)
        event "compose_rm:${MPGS_SERVER_IMAGE:-unset}"
        ;;
      *)
        printf 'unexpected docker compose invocation\n' >&2
        exit 95
        ;;
    esac
    exit 0
    ;;
  run)
    args=$*
    case "$args" in
      *'printf present'*)
        event 'runtime_probe'
        printf 'present'
        ;;
      *'install -d -o mpgs'*)
        event 'prepare_backup_dir'
        ;;
      *'backup-quiesced /var/lib/mpgs/mpgs.db'*)
        event "final_backup:$args"
        if [ "$FAKE_SCENARIO" = quiesced_failure ]; then
          exit 1
        fi
        ;;
      *'recover-steam-leases /var/lib/mpgs/mpgs.db'*)
        event 'recover_leases'
        ;;
      *' backup /var/lib/mpgs/mpgs.db '*'.preflight-'*)
        event "preflight_backup:$args"
        if [ "$FAKE_SCENARIO" = preflight_failure ]; then
          exit 1
        fi
        ;;
      *'rm -f -- '*'.preflight-'*)
        event 'cleanup_preflight'
        ;;
      *'.mpgs-rollback-'*'pre-update-'*)
        backup_path=$(last_arg "$@")
        event "restore_final:$backup_path"
        ;;
      *)
        event "docker_run_other:$args"
        ;;
    esac
    exit 0
    ;;
  rm)
    event "container_rm:$*"
    exit 0
    ;;
  exec)
    event "docker_exec:$*"
    printf '0\n'
    exit 0
    ;;
  *)
    printf 'unexpected fake docker invocation: %s\n' "$*" >&2
    exit 94
    ;;
esac
EOF
  chmod +x "$path"
}

make_fixture() {
  scenario=$1
  fixture=$(mktemp -d)
  mkdir -p "$fixture/root/deploy/runtime" "$fixture/bin"
  cp "$update_script" "$fixture/root/deploy/update.sh"
  cp "$health_helper" "$fixture/root/deploy/deployment-healthcheck-retry.sh"
  cat >"$fixture/root/deploy/materialize-release-compose.sh" <<'EOF'
#!/bin/sh
set -eu
cp "$1/deploy/docker-compose.yml" "$3"
EOF
  chmod +x "$fixture/root/deploy/materialize-release-compose.sh"
  printf 'services: {}\n' >"$fixture/root/deploy/docker-compose.yml"
  cat >"$fixture/root/deploy/.env" <<EOF
MPGS_SERVER_REPOSITORY=example.invalid/mpgs-server
MPGS_WEB_REPOSITORY=example.invalid/mpgs-web
MPGS_RELEASE_SHA=$new_sha
MPGS_DEPLOY_MODE=full
MPGS_BACKUP_RETENTION_COUNT=3
MPGS_DEPLOY_HEALTH_TIMEOUT_SECS=1
MPGS_DEPLOY_QUIESCED_BACKUP_TIMEOUT_SECS=5
EOF
  : >"$fixture/events"
  write_fake_git "$fixture/bin/git"
  write_fake_docker "$fixture/bin/docker"
  write_fake_curl "$fixture/bin/curl"
  write_fake_timeout "$fixture/bin/timeout"
  write_fake_sleep "$fixture/bin/sleep"
  printf '%s\n' "$fixture"
}

run_scenario() {
  scenario=$1
  fixture=$(make_fixture "$scenario")
  output="$fixture/output"
  status=0
  if PATH="$fixture/bin:$PATH" \
    FAKE_EVENTS="$fixture/events" \
    FAKE_SCENARIO="$scenario" \
    FAKE_NEW_SHA="$new_sha" \
    FAKE_OLD_SHA="$old_sha" \
    MPGS_UPDATE_LOCK_HELD=1 \
    MPGS_UPDATE_REEXEC=1 \
    MPGS_UPDATE_SCRIPT_DIR="$fixture/root/deploy" \
    sh "$fixture/root/deploy/update.sh" >"$output" 2>&1; then
    status=0
  else
    status=$?
  fi
  [ "$status" -ne 0 ] || fail "$scenario unexpectedly succeeded"
  SCENARIO_FIXTURE=$fixture
  SCENARIO_EVENTS="$fixture/events"
  SCENARIO_OUTPUT=$output
}

run_scenario preflight_failure
assert_contains "$SCENARIO_EVENTS" 'preflight_backup:'
assert_not_contains "$SCENARIO_EVENTS" 'compose_stop:'
assert_not_contains "$SCENARIO_EVENTS" 'final_backup:'
assert_not_contains "$SCENARIO_EVENTS" 'restore_final:'
assert_contains "$SCENARIO_OUTPUT" 'Online preflight backup failed; leaving the current deployment online.'
rm -rf "$SCENARIO_FIXTURE"

run_scenario quiesced_failure
assert_contains "$SCENARIO_EVENTS" 'preflight_backup:'
assert_contains "$SCENARIO_EVENTS" 'compose_stop:'
assert_contains "$SCENARIO_EVENTS" 'recover_leases'
assert_contains "$SCENARIO_EVENTS" 'final_backup:'
assert_event_contains_all "$SCENARIO_EVENTS" 'compose_up:server=mpgs-rollback-server:' \
  'web=mpgs-rollback-web:' 'mpgs-server' 'mpgs-worker' 'mpgs-web'
assert_not_contains "$SCENARIO_EVENTS" 'restore_final:'
assert_before "$SCENARIO_EVENTS" 'preflight_backup:' 'compose_stop:'
assert_before "$SCENARIO_EVENTS" 'compose_stop:' 'recover_leases'
assert_before "$SCENARIO_EVENTS" 'recover_leases' 'final_backup:'
assert_before "$SCENARIO_EVENTS" 'final_backup:' 'compose_up:server=mpgs-rollback-server:'
assert_contains "$SCENARIO_OUTPUT" 'Quiesced rollback snapshot failed or exceeded 5s; restarting the previous release.'
rm -rf "$SCENARIO_FIXTURE"

run_scenario new_up_failure
assert_contains "$SCENARIO_EVENTS" 'preflight_backup:'
assert_contains "$SCENARIO_EVENTS" 'final_backup:'
assert_event_contains_all "$SCENARIO_EVENTS" \
  "compose_up:server=example.invalid/mpgs-server:sha-$new_sha" \
  "web=example.invalid/mpgs-web:sha-$new_sha" 'mpgs-server' 'mpgs-worker' 'mpgs-web'
assert_contains "$SCENARIO_EVENTS" 'restore_final:'
assert_event_contains_all "$SCENARIO_EVENTS" 'compose_up:server=mpgs-rollback-server:' \
  'web=mpgs-rollback-web:' 'mpgs-server' 'mpgs-worker' 'mpgs-web'
assert_before "$SCENARIO_EVENTS" 'final_backup:' \
  "compose_up:server=example.invalid/mpgs-server:sha-$new_sha"
assert_before "$SCENARIO_EVENTS" \
  "compose_up:server=example.invalid/mpgs-server:sha-$new_sha" 'restore_final:'
assert_before "$SCENARIO_EVENTS" 'restore_final:' 'compose_up:server=mpgs-rollback-server:'
restore_event=$(grep -F 'restore_final:' "$SCENARIO_EVENTS" | head -n 1)
case "$restore_event" in
  *pre-update-*.db) ;;
  *) fail "rollback did not restore the exact pre-update snapshot: $restore_event" ;;
esac
case "$restore_event" in
  *.preflight-*) fail "rollback incorrectly used the online preflight artifact" ;;
esac
assert_contains "$SCENARIO_OUTPUT" 'Previous release restored.'
rm -rf "$SCENARIO_FIXTURE"

run_scenario health_timeout
assert_event_contains_all "$SCENARIO_EVENTS" \
  "compose_up:server=example.invalid/mpgs-server:sha-$new_sha" \
  "web=example.invalid/mpgs-web:sha-$new_sha" 'mpgs-server' 'mpgs-worker' 'mpgs-web'
assert_contains "$SCENARIO_EVENTS" 'curl:'
assert_contains "$SCENARIO_EVENTS" 'restore_final:'
assert_event_contains_all "$SCENARIO_EVENTS" 'compose_up:server=mpgs-rollback-server:' \
  'web=mpgs-rollback-web:' 'mpgs-server' 'mpgs-worker' 'mpgs-web'
assert_before "$SCENARIO_EVENTS" 'final_backup:' \
  "compose_up:server=example.invalid/mpgs-server:sha-$new_sha"
assert_before "$SCENARIO_EVENTS" \
  "compose_up:server=example.invalid/mpgs-server:sha-$new_sha" 'curl:'
assert_before "$SCENARIO_EVENTS" 'curl:' 'restore_final:'
assert_before "$SCENARIO_EVENTS" 'restore_final:' 'compose_up:server=mpgs-rollback-server:'
assert_contains "$SCENARIO_OUTPUT" 'Deployment did not become healthy after 1 attempts:'
assert_contains "$SCENARIO_OUTPUT" 'Previous release restored.'
rm -rf "$SCENARIO_FIXTURE"

printf 'update failure-injection tests passed\n'
