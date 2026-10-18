#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
update_script="$script_dir/update.sh"

sh -n "$update_script"

line_of() {
  needle=$1
  awk -v needle="$needle" 'index($0, needle) { print NR; exit }' "$update_script"
}

preflight_line=$(line_of 'backup /var/lib/mpgs/mpgs.db "/var/lib/mpgs/$preflight_backup_rel"')
stop_line=$(line_of 'if ! old_compose stop $stop_services; then')
final_line=$(line_of 'backup-quiesced /var/lib/mpgs/mpgs.db "/var/lib/mpgs/$backup_rel"')
timeout_line=$(line_of 'if ! timeout "$quiesced_backup_timeout_secs" docker run')
report_line=$(awk '
  {
    line=$0
    sub(/\r$/, "", line)
    if (line ~ /^[[:space:]]*report_quiesce_window[[:space:]]*$/) {
      print NR
      exit
    }
  }
' "$update_script")

for value in "$preflight_line" "$stop_line" "$final_line" "$timeout_line" "$report_line"; do
  case "$value" in
    ''|*[!0-9]*)
      printf 'required update.sh backup/cutover marker is missing\n' >&2
      exit 1
      ;;
  esac
done

if [ "$preflight_line" -ge "$stop_line" ]; then
  printf 'full verified preflight backup must run before services are stopped\n' >&2
  exit 1
fi
if [ "$final_line" -le "$stop_line" ]; then
  printf 'exact quiesced rollback snapshot must run after services are stopped\n' >&2
  exit 1
fi
if [ "$timeout_line" -ge "$final_line" ]; then
  printf 'quiesced rollback snapshot must be wrapped by the configured timeout\n' >&2
  exit 1
fi
if [ "$report_line" -le "$final_line" ]; then
  printf 'cutover duration must be reported after the final snapshot and health validation\n' >&2
  exit 1
fi

if ! awk '
  index($0, "if ! timeout \"$quiesced_backup_timeout_secs\" docker run") {
    in_final=1
    remaining=8
  }
  in_final {
    if (index($0, "\"$new_server_image\"")) new_image=1
    if (index($0, "backup-quiesced /var/lib/mpgs/mpgs.db")) {
      exit(new_image ? 0 : 1)
    }
    remaining--
    if (remaining <= 0) exit 1
  }
  END { if (!in_final) exit 1 }
' "$update_script"; then
  printf 'backup-quiesced must run from the incoming image so first deployment can use the command\n' >&2
  exit 1
fi

grep -F 'MPGS_DEPLOY_QUIESCED_BACKUP_TIMEOUT_SECS' "$update_script" >/dev/null
grep -F 'preflight_backup_created=1' "$update_script" >/dev/null
grep -F 'cleanup_preflight_backup' "$update_script" >/dev/null
grep -F 'docker rm -f "$quiesced_backup_container"' "$update_script" >/dev/null

printf 'update backup cutover tests passed\n'
