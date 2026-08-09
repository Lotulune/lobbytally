#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$script_dir/deployment-healthcheck-retry.sh"

classify_worker_health_result 0
if classify_worker_health_result 3; then
  printf 'soft-stale worker health unexpectedly passed\n' >&2
  exit 1
else
  test "$?" -eq 3
fi
if classify_worker_health_result 7; then
  printf 'hard-unhealthy worker health unexpectedly passed\n' >&2
  exit 1
else
  test "$?" -eq 1
fi

attempts=0
results="1 1 0"
deployment_healthcheck() {
  attempts=$((attempts + 1))
  result=${results%% *}
  if [ "$results" = "$result" ]; then
    results=
  else
    results=${results#* }
  fi
  deployment_health_detail="fixture result $result"
  return "$result"
}
retry_deployment_healthcheck fixture 5 0
test "$attempts" -eq 3

attempts=0
results="2 0"
if retry_deployment_healthcheck fixture 5 0; then
  printf 'revision mismatch unexpectedly passed\n' >&2
  exit 1
else
  result=$?
fi
test "$result" -eq 2
test "$attempts" -eq 1

attempts=0
results="1 1 1"
if retry_deployment_healthcheck fixture 3 0; then
  printf 'exhausted health check unexpectedly passed\n' >&2
  exit 1
else
  result=$?
fi
test "$result" -eq 1
test "$attempts" -eq 3

attempts=0
results="3 3 3"
if retry_deployment_healthcheck fixture 3 0; then
  printf 'stale worker health unexpectedly passed\n' >&2
  exit 1
else
  result=$?
fi
test "$result" -eq 3
test "$attempts" -eq 3

printf 'deployment healthcheck retry tests passed\n'
