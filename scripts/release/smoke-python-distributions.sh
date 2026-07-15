#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <artifact-dir> <version>" >&2
  exit 2
fi

artifact_dir=$(cd "$1" && pwd)
version=$2
build_constraints=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/python-build-constraints.txt
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
  echo "version must be a stable x.y.z version" >&2
  exit 2
}

shopt -s nullglob
wheels=("$artifact_dir"/*.whl)
sdists=("$artifact_dir"/*.tar.gz)
if (( ${#wheels[@]} != 1 || ${#sdists[@]} != 1 )); then
  echo "expected exactly one Python wheel and one sdist in $artifact_dir" >&2
  printf 'wheel: %s\n' "${wheels[@]:-<none>}" >&2
  printf 'sdist: %s\n' "${sdists[@]:-<none>}" >&2
  exit 1
fi

smoke() {
  local artifact=$1
  local kind=$2
  local environment
  environment=$(mktemp -d "${TMPDIR:-/tmp}/rspyts-python-${kind}.XXXXXX")
  trap 'rm -rf "$environment"' RETURN

  uv venv --python 3.11 "$environment"
  UV_BUILD_CONSTRAINT="$build_constraints" \
    uv pip install --python "$environment/bin/python" "$artifact"
  (
    cd "${TMPDIR:-/tmp}"
    env -u PYTHONPATH \
      RSPYTS_EXPECTED_VERSION="$version" \
      "$environment/bin/python" - <<'PY'
import importlib.metadata
import os

import rspyts
import rspyts._internal as internal

expected = os.environ["RSPYTS_EXPECTED_VERSION"]
assert importlib.metadata.version("rspyts") == expected
assert rspyts.__version__ == expected
internal.require_emitter_api(internal.EMITTER_API_VERSION)
PY
  )
}

smoke "${wheels[0]}" wheel
smoke "${sdists[0]}" sdist
