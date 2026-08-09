#!/bin/sh
set -eu

test_dir=$(mktemp -d "${TMPDIR:-/tmp}/mpgs-release-compose-test.XXXXXX")
trap 'rm -rf -- "$test_dir"' EXIT HUP INT TERM

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
origin_repo="$test_dir/origin.git"
source_repo="$test_dir/source"
shallow_repo="$test_dir/shallow"
output_file="$test_dir/docker-compose.yml"

git init --bare --quiet "$origin_repo"
git init --quiet "$source_repo"
git -C "$source_repo" config user.name "MPGS deploy test"
git -C "$source_repo" config user.email "deploy-test@example.invalid"
git -C "$source_repo" remote add origin "$origin_repo"
mkdir -p "$source_repo/deploy"
printf 'services:\n  pinned-service:\n    image: pinned\n' \
  >"$source_repo/deploy/docker-compose.yml"
git -C "$source_repo" add deploy/docker-compose.yml
git -C "$source_repo" commit --quiet -m "pinned release"
pinned_sha=$(git -C "$source_repo" rev-parse HEAD)
git -C "$source_repo" branch -M main
git -C "$source_repo" push --quiet -u origin main

printf 'services:\n  current-service:\n    image: current\n' \
  >"$source_repo/deploy/docker-compose.yml"
git -C "$source_repo" commit --quiet -am "current release"
current_sha=$(git -C "$source_repo" rev-parse HEAD)
git -C "$source_repo" push --quiet origin main

sh "$script_dir/materialize-release-compose.sh" \
  "$source_repo" "$pinned_sha" "$output_file"
grep -F 'pinned-service:' "$output_file" >/dev/null
if grep -F 'current-service:' "$output_file" >/dev/null; then
  printf 'materialized Compose came from the current checkout\n' >&2
  exit 1
fi
if [ "$(git -C "$source_repo" rev-parse HEAD)" != "$current_sha" ]; then
  printf 'materialization changed the source checkout\n' >&2
  exit 1
fi

git clone --quiet --depth 1 --branch main "file://$origin_repo" "$shallow_repo"
if git -C "$shallow_repo" cat-file -e "${pinned_sha}^{commit}" 2>/dev/null; then
  printf 'shallow test clone unexpectedly contains the pinned commit\n' >&2
  exit 1
fi
sh "$script_dir/materialize-release-compose.sh" \
  "$shallow_repo" "$pinned_sha" "$output_file"
grep -F 'pinned-service:' "$output_file" >/dev/null
if [ "$(git -C "$shallow_repo" rev-parse HEAD)" != "$current_sha" ]; then
  printf 'fetching a missing pin changed the source checkout\n' >&2
  exit 1
fi

printf 'materialize-release-compose tests passed\n'
