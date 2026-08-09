#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
  printf 'usage: %s <repo-root> <release-sha> <output-file>\n' "$0" >&2
  exit 2
fi

repo_root=$1
release_sha=$2
output_file=$3

case "$release_sha" in
  ''|*[!0-9a-f]*)
    printf 'release SHA must be 40 lowercase hexadecimal characters\n' >&2
    exit 2
    ;;
esac
if [ "${#release_sha}" -ne 40 ]; then
  printf 'release SHA must contain 40 hexadecimal characters\n' >&2
  exit 2
fi

if ! git -C "$repo_root" cat-file -e "${release_sha}^{commit}" 2>/dev/null; then
  printf 'Fetching release commit %s from origin.\n' "$release_sha"
  git -C "$repo_root" fetch --no-tags origin "$release_sha"
fi

resolved_sha=$(git -C "$repo_root" rev-parse --verify "${release_sha}^{commit}")
if [ "$resolved_sha" != "$release_sha" ]; then
  printf 'resolved release commit %s does not match requested SHA %s\n' \
    "$resolved_sha" "$release_sha" >&2
  exit 1
fi

output_tmp="${output_file}.tmp.$$"
trap 'rm -f -- "$output_tmp"' EXIT HUP INT TERM
git -C "$repo_root" show "${release_sha}:deploy/docker-compose.yml" >"$output_tmp"
mv -- "$output_tmp" "$output_file"
trap - EXIT HUP INT TERM
