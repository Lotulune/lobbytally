#!/bin/sh
set -eu

interval="${MPGS_WORKER_INTERVAL_SECS:-60}"
job_limit="${MPGS_WORKER_JOB_LIMIT:-10}"
enrich_limit="${MPGS_WORKER_ENRICH_LIMIT:-100}"
max_failures="${MPGS_WORKER_MAX_CONSECUTIVE_FAILURES:-5}"
max_run_secs="${MPGS_WORKER_MAX_RUN_SECS:-1800}"
retry_interval="${MPGS_WORKER_RETRY_INTERVAL_SECS:-30}"
health_file="${MPGS_WORKER_HEALTH_FILE:-/var/lib/mpgs/.worker-health}"

require_positive_integer() {
    name=$1
    value=$2
    case "$value" in
        ''|*[!0-9]*|0)
            printf '%s must be a positive integer (got: %s)\n' "$name" "$value" >&2
            exit 2
            ;;
    esac
}

require_positive_integer MPGS_WORKER_INTERVAL_SECS "$interval"
require_positive_integer MPGS_WORKER_JOB_LIMIT "$job_limit"
require_positive_integer MPGS_WORKER_ENRICH_LIMIT "$enrich_limit"
require_positive_integer MPGS_WORKER_MAX_CONSECUTIVE_FAILURES "$max_failures"
require_positive_integer MPGS_WORKER_MAX_RUN_SECS "$max_run_secs"
require_positive_integer MPGS_WORKER_RETRY_INTERVAL_SECS "$retry_interval"

write_health() {
    status=$1
    timestamp=$2
    failures=$3
    health_tmp="${health_file}.tmp.$$"
    umask 077
    printf '%s %s %s\n' "$status" "$timestamp" "$failures" >"$health_tmp"
    mv -f "$health_tmp" "$health_file"
}

if [ "${1:-}" = "--healthcheck" ]; then
    [ -r "$health_file" ] || exit 1
    IFS=' ' read -r status timestamp failures <"$health_file" || exit 1
    case "$timestamp" in
        ''|*[!0-9]*) exit 1 ;;
    esac
    now=$(date +%s)
    age=$((now - timestamp))
    [ "$age" -ge 0 ] || exit 1
    case "$status" in
        running)
            [ "$age" -le "$max_run_secs" ]
            ;;
        ok)
            max_ok_age=$((interval * 3 + 60))
            [ "$age" -le "$max_ok_age" ]
            ;;
        *)
            exit 1
            ;;
    esac
    exit
fi

failures=0
while :; do
    sleep_secs="$interval"
    started_at=$(date +%s)
    write_health running "$started_at" "$failures"
    if /usr/local/bin/mpgs-dbtool run-steam-worker-once \
        /var/lib/mpgs/mpgs.db "$job_limit" "$enrich_limit"; then
        failures=0
        write_health ok "$(date +%s)" "$failures"
    else
        failures=$((failures + 1))
        write_health error "$(date +%s)" "$failures"
        printf 'mpgs worker attempt failed (%s/%s)\n' "$failures" "$max_failures" >&2
        if [ "$failures" -ge "$max_failures" ]; then
            printf 'mpgs worker stopped after %s consecutive failures\n' "$failures" >&2
            exit 1
        fi
        sleep_secs="$retry_interval"
    fi
    sleep "$sleep_secs"
done
