#!/bin/sh
set -u

before_sha=${1:-}
after_sha=${2:-}
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
classifier="$script_dir/release_scope.sh"
zero_sha=0000000000000000000000000000000000000000

# This helper must fail safe toward publishing. It therefore always exits 0
# with a boolean result, even when history retrieval or classification fails.
if [ -z "$before_sha" ] || [ -z "$after_sha" ] || [ "$before_sha" = "$zero_sha" ]; then
  printf 'release scope fallback: missing/initial comparison SHA; publishing\n' >&2
  printf 'true\n'
  exit 0
fi

if ! git cat-file -e "$after_sha^{commit}" 2>/dev/null; then
  printf 'release scope fallback: target SHA is unavailable; publishing\n' >&2
  printf 'true\n'
  exit 0
fi

if ! git cat-file -e "$before_sha^{commit}" 2>/dev/null; then
  if ! git fetch --no-tags --depth=1 origin "$before_sha" >/dev/null 2>&1; then
    printf 'release scope fallback: previous SHA could not be fetched; publishing\n' >&2
    printf 'true\n'
    exit 0
  fi
fi

# Disable rename detection deliberately. A production file renamed into an
# exempt directory must expose both the removed production path and the new
# exempt path; classifying only the rename destination could suppress release.
if ! changed_paths=$(git diff --no-renames --name-only "$before_sha" "$after_sha" -- 2>/dev/null); then
  printf 'release scope fallback: changed paths could not be computed; publishing\n' >&2
  printf 'true\n'
  exit 0
fi

if ! result=$(printf '%s\n' "$changed_paths" | sh "$classifier"); then
  printf 'release scope fallback: classifier failed; publishing\n' >&2
  printf 'true\n'
  exit 0
fi

case "$result" in
  true|false)
    printf '%s\n' "$result"
    ;;
  *)
    printf 'release scope fallback: invalid classifier result; publishing\n' >&2
    printf 'true\n'
    ;;
esac
