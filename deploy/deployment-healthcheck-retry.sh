#!/bin/sh

classify_worker_health_result() {
  case "$1" in
    0) return 0 ;;
    3) return 3 ;;
    *) return 1 ;;
  esac
}

retry_deployment_healthcheck() {
  compose_runner=$1
  max_attempts=$2
  delay_secs=$3
  attempt=1
  last_health_result=1
  while [ "$attempt" -le "$max_attempts" ]; do
    health_result=0
    deployment_healthcheck "$compose_runner" || health_result=$?
    last_health_result=$health_result
    if [ "$health_result" -eq 0 ]; then
      return 0
    fi
    printf 'Existing deployment health check attempt %s/%s failed: %s\n' \
      "$attempt" "$max_attempts" "${deployment_health_detail:-unknown failure}" >&2
    if [ "$health_result" -eq 2 ]; then
      return 2
    fi
    if [ "$attempt" -lt "$max_attempts" ] && [ "$delay_secs" -gt 0 ]; then
      sleep "$delay_secs"
    fi
    attempt=$((attempt + 1))
  done
  return "$last_health_result"
}
