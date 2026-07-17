#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "usage: $0 PACKAGE_DIRECTORY" >&2
  exit 2
fi

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
temporary=$(mktemp -d "${TMPDIR:-/tmp}/rspyts-npm-package.XXXXXX")
trap 'rm -rf "$temporary"' EXIT

packed=$(npm pack --json --pack-destination "$temporary" "$1")
filename=$(node -e \
  'const value = JSON.parse(process.argv[1]); process.stdout.write(value[0].filename);' \
  "$packed")
mkdir "$temporary/extracted"
tar -xzf "$temporary/$filename" -C "$temporary/extracted"
"$repository_root/scripts/release/verify-private-data.sh" \
  "$temporary/extracted"
