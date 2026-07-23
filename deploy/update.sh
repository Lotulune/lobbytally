#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
if [ -f "$script_dir/.env" ]; then
  set -a
  # deploy/.env is the administrator-owned Compose configuration.
  . "$script_dir/.env"
  set +a
fi
branch=${MPGS_DEPLOY_BRANCH:-main}
mode=${MPGS_DEPLOY_MODE:-full}

case "$mode" in
  backend)
    services="mpgs-server mpgs-worker"
    health_port=${MPGS_API_PORT:-18081}
    ;;
  full)
    services="mpgs-server mpgs-worker mpgs-web"
    health_port=18082
    ;;
  *)
    printf 'MPGS_DEPLOY_MODE must be backend or full (got: %s)\n' "$mode" >&2
    exit 2
    ;;
esac

cd "$repo_root"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git pull --ff-only origin "$branch"
else
  printf 'Source directory is not a Git checkout; pulling container images only.\n'
fi

docker compose --env-file deploy/.env -f deploy/docker-compose.yml pull $services
docker compose --env-file deploy/.env -f deploy/docker-compose.yml \
  up -d --no-build --remove-orphans $services

docker compose --env-file deploy/.env -f deploy/docker-compose.yml \
  exec -T mpgs-server mpgs-dbtool integrity /var/lib/mpgs/mpgs.db
curl --fail --silent --show-error "http://127.0.0.1:${health_port}/health/ready"
printf '\nMPGS %s deployment updated from %s\n' "$mode" "$branch"
