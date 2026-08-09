#!/bin/sh
set -eu

interval="${MPGS_WORKER_INTERVAL_SECS:-60}"
job_limit="${MPGS_WORKER_JOB_LIMIT:-1}"
enrich_limit="${MPGS_WORKER_ENRICH_LIMIT:-20}"
max_failures="${MPGS_WORKER_MAX_CONSECUTIVE_FAILURES:-5}"
max_run_secs="${MPGS_WORKER_MAX_RUN_SECS:-1800}"
retry_interval="${MPGS_WORKER_RETRY_INTERVAL_SECS:-30}"
heartbeat_interval="${MPGS_WORKER_HEARTBEAT_INTERVAL_SECS:-10}"
heartbeat_grace="${MPGS_WORKER_HEARTBEAT_GRACE_SECS:-30}"
kill_grace="${MPGS_WORKER_KILL_GRACE_SECS:-10}"
health_file="${MPGS_WORKER_HEALTH_FILE:-/var/lib/mpgs/.worker-health}"
dbtool="${MPGS_WORKER_DBTOOL_PATH:-/usr/local/bin/mpgs-dbtool}"

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
require_positive_integer MPGS_WORKER_HEARTBEAT_INTERVAL_SECS "$heartbeat_interval"
require_positive_integer MPGS_WORKER_HEARTBEAT_GRACE_SECS "$heartbeat_grace"
require_positive_integer MPGS_WORKER_KILL_GRACE_SECS "$kill_grace"

write_health() {
    wh_status=$1
    wh_heartbeat_at=$2
    wh_run_started_at=$3
    wh_failures=$4
    wh_child_pid=$5
    health_tmp="${health_file}.tmp.$$"
    umask 077
    printf 'v2 %s %s %s %s %s\n' \
        "$wh_status" "$wh_heartbeat_at" "$wh_run_started_at" "$wh_failures" \
        "$wh_child_pid" >"$health_tmp"
    mv -f "$health_tmp" "$health_file"
}

if [ "${1:-}" = "--healthcheck" ]; then
    [ -r "$health_file" ] || exit 1
    IFS=' ' read -r version status heartbeat_at run_started_at failures child_pid extra \
        <"$health_file" || exit 1
    [ "$version" = v2 ] || exit 1
    [ -z "${extra:-}" ] || exit 1
    for value in "$heartbeat_at" "$run_started_at" "$failures" "$child_pid"; do
        case "$value" in
            ''|*[!0-9]*) exit 1 ;;
        esac
    done
    now=$(date +%s)
    heartbeat_age=$((now - heartbeat_at))
    [ "$heartbeat_age" -ge 0 ] || exit 1
    max_heartbeat_age=$((heartbeat_interval * 3))

    case "$status" in
        starting)
            [ "$run_started_at" -eq 0 ] || exit 1
            [ "$child_pid" -eq 0 ] || exit 1
            ;;
        running)
            [ "$run_started_at" -gt 0 ] || exit 1
            [ "$child_pid" -gt 0 ] || exit 1
            run_age=$((now - run_started_at))
            [ "$run_age" -ge 0 ] || exit 1
            [ "$run_age" -le "$max_run_secs" ] || exit 1
            kill -0 "$child_pid" 2>/dev/null || exit 1
            ;;
        ok)
            [ "$run_started_at" -eq 0 ] || exit 1
            [ "$child_pid" -eq 0 ] || exit 1
            ;;
        error|*)
            exit 1
            ;;
    esac

    if [ "$heartbeat_age" -le "$max_heartbeat_age" ]; then
        exit 0
    fi
    if [ "$heartbeat_age" -le $((max_heartbeat_age + heartbeat_grace)) ]; then
        exit 3
    fi
    exit 1
fi

terminate_child() {
    terminated_pid=$1
    kill -TERM "$terminated_pid" 2>/dev/null || true
    remaining=$kill_grace
    while kill -0 "$terminated_pid" 2>/dev/null && [ "$remaining" -gt 0 ]; do
        sleep 1
        remaining=$((remaining - 1))
    done
    if kill -0 "$terminated_pid" 2>/dev/null; then
        kill -KILL "$terminated_pid" 2>/dev/null || true
    fi
}

monitor_child() {
    pid=$1
    run_started=$2
    watchdog_flag=$3
    while kill -0 "$pid" 2>/dev/null; do
        now=$(date +%s)
        if [ $((now - run_started)) -ge "$max_run_secs" ]; then
            : >"$watchdog_flag"
            terminate_child "$pid"
            return
        fi
        write_health running "$now" "$run_started" "$failures" "$pid"
        sleep "$heartbeat_interval"
    done
}

sleep_while_healthy() {
    remaining=$1
    while [ "$remaining" -gt 0 ]; do
        step=$heartbeat_interval
        if [ "$step" -gt "$remaining" ]; then
            step=$remaining
        fi
        sleep "$step"
        remaining=$((remaining - step))
        write_health ok "$(date +%s)" 0 "$failures" 0
    done
}

failures=0
write_health starting "$(date +%s)" 0 "$failures" 0
while :; do
    sleep_secs="$interval"
    started_at=$(date +%s)
    "$dbtool" run-steam-worker-once \
        /var/lib/mpgs/mpgs.db "$job_limit" "$enrich_limit" &
    child_pid=$!
    write_health running "$started_at" "$started_at" "$failures" "$child_pid"

    watchdog_flag="${health_file}.watchdog.$$"
    rm -f -- "$watchdog_flag"
    monitor_child "$child_pid" "$started_at" "$watchdog_flag" &
    monitor_pid=$!
    child_succeeded=0
    if wait "$child_pid"; then
        child_succeeded=1
    fi
    kill "$monitor_pid" 2>/dev/null || true
    wait "$monitor_pid" 2>/dev/null || true

    if [ -f "$watchdog_flag" ]; then
        rm -f -- "$watchdog_flag"
        failures=$((failures + 1))
        write_health error "$(date +%s)" "$started_at" "$failures" 0
        printf 'mpgs worker exceeded hard watchdog (%ss); child %s terminated\n' \
            "$max_run_secs" "$child_pid" >&2
        exit 1
    fi

    if [ "$child_succeeded" -eq 1 ]; then
        failures=0
        write_health ok "$(date +%s)" 0 "$failures" 0
        sleep_while_healthy "$sleep_secs"
    else
        failures=$((failures + 1))
        write_health error "$(date +%s)" "$started_at" "$failures" 0
        printf 'mpgs worker attempt failed (%s/%s)\n' "$failures" "$max_failures" >&2
        if [ "$failures" -ge "$max_failures" ]; then
            printf 'mpgs worker stopped after %s consecutive failures\n' "$failures" >&2
            exit 1
        fi
        sleep_secs="$retry_interval"
        sleep "$sleep_secs"
    fi
done
