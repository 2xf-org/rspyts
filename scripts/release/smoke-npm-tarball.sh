#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <artifact-dir> <version>" >&2
  exit 2
fi

artifact_dir=$(cd "$1" && pwd)
version=$2
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
  echo "version must be a stable x.y.z version" >&2
  exit 2
}

shopt -s nullglob
tarballs=("$artifact_dir"/*.tgz)
if (( ${#tarballs[@]} != 1 )); then
  echo "expected exactly one npm tarball in $artifact_dir" >&2
  printf 'tarball: %s\n' "${tarballs[@]:-<none>}" >&2
  exit 1
fi

consumer=$(mktemp -d "${TMPDIR:-/tmp}/rspyts-npm-consumer.XXXXXX")
trap 'rm -rf "$consumer"' EXIT
cd "$consumer"
npm init --yes >/dev/null
npm install --ignore-scripts --no-audit --no-fund "${tarballs[0]}"

RSPYTS_EXPECTED_VERSION="$version" node --input-type=module <<'JS'
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const root = await import("rspyts");
const internal = await import("rspyts/internal/abi3");
const metadata = JSON.parse(await readFile("node_modules/rspyts/package.json", "utf8"));

assert.equal(metadata.version, process.env.RSPYTS_EXPECTED_VERSION);
assert.equal(typeof root, "object");
assert.equal(typeof internal.callFn, "function");
JS
