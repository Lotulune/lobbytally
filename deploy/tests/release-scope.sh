#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
classifier="$repo_root/scripts/release_scope.sh"

scope() {
  printf '%s\n' "$@" | sh "$classifier" 2>/dev/null
}

expect_scope() {
  expected=$1
  shift
  actual=$(scope "$@")
  if [ "$actual" != "$expected" ]; then
    printf 'release-scope mismatch: expected=%s actual=%s paths=%s\n' \
      "$expected" "$actual" "$*" >&2
    exit 1
  fi
}

# Clearly non-production changes must not move release-main.
expect_scope false docs/OPERATIONS.md
expect_scope false .github/workflows/ci.yml deploy/tests/update-failure-injection.sh
expect_scope false apps/desktop/src-tauri/tauri.conf.json e2e-tests/package.json
expect_scope false packaging/linux/install.sh scripts/package_server.ps1
expect_scope false deploy/.env.example deploy/mpgs-update.service deploy/mpgs-host.nginx.conf
expect_scope false README.md .gitignore rustfmt.toml

# Every file that can change the server/web images or automatic host cutover
# remains release-scoped.
expect_scope true Cargo.toml
expect_scope true Cargo.lock
expect_scope true rust-toolchain.toml
expect_scope true Dockerfile
expect_scope true .dockerignore
expect_scope true apps/server/src/main.rs
expect_scope true apps/dbtool/src/main.rs
expect_scope true crates/storage/src/backup.rs
expect_scope true migrations/0031_release_transition_refresh.sql
expect_scope true web/src/App.tsx
expect_scope true package.json pnpm-lock.yaml pnpm-workspace.yaml
expect_scope true deploy/docker-compose.yml
expect_scope true deploy/update.sh
expect_scope true deploy/deployment-healthcheck-retry.sh
expect_scope true deploy/materialize-release-compose.sh
expect_scope true deploy/mpgs-worker-loop.sh
expect_scope true deploy/mpgs-web.nginx.conf

# Mixed changes and newly introduced/unclassified paths fail safe to release.
expect_scope true docs/OPERATIONS.md crates/domain/src/lib.rs
expect_scope true future-production-input.bin

printf 'release scope tests passed\n'
