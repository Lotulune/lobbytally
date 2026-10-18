#!/bin/sh
set -eu

# Read repository-relative changed paths from stdin and print exactly one
# boolean. The safe default is to release: only paths that are explicitly
# known not to affect the production server/web runtime are exempted.
runtime_changed=false

while IFS= read -r path || [ -n "$path" ]; do
  [ -n "$path" ] || continue
  case "$path" in
    .github/* | \
    docs/* | \
    apps/desktop/* | \
    e2e-tests/* | \
    packaging/* | \
    scripts/* | \
    deploy/tests/* | \
    deploy/.env.example | \
    deploy/mpgs.env.example | \
    deploy/mpgs-update.service | \
    deploy/mpgs-update.timer | \
    deploy/mpgs-api-host.nginx.conf | \
    deploy/mpgs-host.nginx.conf | \
    README.md | \
    README.en.md | \
    .gitignore | \
    rustfmt.toml)
      ;;
    *)
      runtime_changed=true
      printf 'production-runtime path changed: %s\n' "$path" >&2
      ;;
  esac
done

printf '%s\n' "$runtime_changed"
