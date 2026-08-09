#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
worker="$repo_root/deploy/mpgs-worker-loop.sh"
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/mpgs-worker-health-test.XXXXXX")
cleanup() {
  rm -rf -- "$test_dir"
}
trap cleanup EXIT HUP INT TERM

health_file="$test_dir/health"
now=$(date +%s)
run_healthcheck() {
  MPGS_WORKER_HEALTH_FILE="$health_file" \
  MPGS_WORKER_INTERVAL_SECS=60 \
  MPGS_WORKER_HEARTBEAT_INTERVAL_SECS=1 \
  MPGS_WORKER_HEARTBEAT_GRACE_SECS=3 \
  MPGS_WORKER_MAX_RUN_SECS=10 \
    sh "$worker" --healthcheck
}
expect_result() {
  expected=$1
  shift
  actual=0
  "$@" || actual=$?
  if [ "$actual" -ne "$expected" ]; then
    printf 'expected exit %s, got %s\n' "$expected" "$actual" >&2
    exit 1
  fi
}

expect_result 1 run_healthcheck
printf 'v2 running %s' "$now" >"$health_file"
expect_result 1 run_healthcheck
printf 'v2 ok nope 0 0 0\n' >"$health_file"
expect_result 1 run_healthcheck
printf 'v2 error %s 0 1 0\n' "$now" >"$health_file"
expect_result 1 run_healthcheck
printf 'v2 ok %s 0 0 0\n' "$now" >"$health_file"
expect_result 0 run_healthcheck
printf 'v2 ok %s 0 0 0\n' "$((now - 4))" >"$health_file"
expect_result 3 run_healthcheck
printf 'v2 ok %s 0 0 0\n' "$((now - 7))" >"$health_file"
expect_result 1 run_healthcheck
printf 'v2 ok %s 0 0 0\n' "$((now + 30))" >"$health_file"
expect_result 1 run_healthcheck
printf 'v2 running %s %s 0 %s\n' "$now" "$now" "$$" >"$health_file"
expect_result 0 run_healthcheck
printf 'v2 running %s %s 0 %s\n' "$now" "$((now - 11))" "$$" >"$health_file"
expect_result 1 run_healthcheck
printf 'v2 running %s %s 0 999999\n' "$now" "$now" >"$health_file"
expect_result 1 run_healthcheck

missing_dbtool="$test_dir/missing-dbtool"
expect_result 1 env \
  MPGS_WORKER_HEALTH_FILE="$health_file" \
  MPGS_WORKER_DBTOOL_PATH="$missing_dbtool" \
  MPGS_WORKER_INTERVAL_SECS=1 \
  MPGS_WORKER_RETRY_INTERVAL_SECS=1 \
  MPGS_WORKER_HEARTBEAT_INTERVAL_SECS=1 \
  MPGS_WORKER_HEARTBEAT_GRACE_SECS=1 \
  MPGS_WORKER_KILL_GRACE_SECS=1 \
  MPGS_WORKER_MAX_RUN_SECS=3 \
  MPGS_WORKER_MAX_CONSECUTIVE_FAILURES=1 \
  sh "$worker"

slow_dbtool="$test_dir/slow-dbtool"
printf '#!/bin/sh\nexec sleep 30\n' >"$slow_dbtool"
chmod +x "$slow_dbtool"
expect_result 1 env \
  MPGS_WORKER_HEALTH_FILE="$health_file" \
  MPGS_WORKER_DBTOOL_PATH="$slow_dbtool" \
  MPGS_WORKER_INTERVAL_SECS=1 \
  MPGS_WORKER_RETRY_INTERVAL_SECS=1 \
  MPGS_WORKER_HEARTBEAT_INTERVAL_SECS=1 \
  MPGS_WORKER_HEARTBEAT_GRACE_SECS=1 \
  MPGS_WORKER_KILL_GRACE_SECS=1 \
  MPGS_WORKER_MAX_RUN_SECS=1 \
  MPGS_WORKER_MAX_CONSECUTIVE_FAILURES=5 \
  sh "$worker"
grep -F 'v2 error ' "$health_file" >/dev/null

printf 'worker health v2 tests passed\n'
