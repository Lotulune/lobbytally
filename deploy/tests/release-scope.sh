#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
classifier="$repo_root/scripts/release_scope.sh"
git_classifier="$repo_root/scripts/release_scope_from_git.sh"

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
expect_scope false README.en.md
expect_scope false README.md README.en.md docs/images/lobbytally-feed-zh.png
expect_scope true README.en.md web/src/App.tsx

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

fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT HUP INT TERM
git -C "$fixture" init -q
git -C "$fixture" config user.name release-scope-test
git -C "$fixture" config user.email release-scope-test@example.invalid

mkdir -p "$fixture/docs" "$fixture/deploy"
printf 'baseline\n' >"$fixture/docs/notes.md"
printf 'runtime\n' >"$fixture/deploy/update.sh"
git -C "$fixture" add .
git -C "$fixture" commit -qm baseline
baseline=$(git -C "$fixture" rev-parse HEAD)

# A docs-only commit including a new English README stays non-runtime.
printf 'docs only\n' >>"$fixture/docs/notes.md"
printf 'English documentation\n' >"$fixture/README.en.md"
git -C "$fixture" add docs/notes.md README.en.md
git -C "$fixture" commit -qm docs-only
docs_commit=$(git -C "$fixture" rev-parse HEAD)
actual=$(CDPATH= cd -- "$fixture" && sh "$git_classifier" "$baseline" "$docs_commit" 2>/dev/null)
[ "$actual" = false ] || {
  printf 'git release scope should ignore docs-only commit, got %s\n' "$actual" >&2
  exit 1
}

# Renaming a production path into an exempt directory must still release. The
# --no-renames diff exposes the deleted deploy/update.sh path as well as the
# new docs path.
git -C "$fixture" mv deploy/update.sh docs/old-update.sh
git -C "$fixture" commit -qm rename-runtime-into-docs
rename_commit=$(git -C "$fixture" rev-parse HEAD)
actual=$(CDPATH= cd -- "$fixture" && sh "$git_classifier" "$docs_commit" "$rename_commit" 2>/dev/null)
[ "$actual" = true ] || {
  printf 'runtime-to-docs rename must release, got %s\n' "$actual" >&2
  exit 1
}

# Missing history/fetch errors must also fail safe to publishing. The fixture
# intentionally has no origin remote.
actual=$(CDPATH= cd -- "$fixture" && sh "$git_classifier" \
  1111111111111111111111111111111111111111 "$rename_commit" 2>/dev/null)
[ "$actual" = true ] || {
  printf 'missing previous SHA must fail safe to release, got %s\n' "$actual" >&2
  exit 1
}

printf 'release scope tests passed\n'
