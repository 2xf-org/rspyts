#!/usr/bin/env bash

set -euo pipefail

usage() {
  echo "usage: $0 [--allow-dirty] [version]" >&2
  exit 2
}

allow_dirty=false
if [[ ${1:-} == "--allow-dirty" ]]; then
  allow_dirty=true
  shift
fi
[[ $# -le 1 ]] || usage

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repository_root"

workspace_version=$(
  cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.name == "rspyts") | .version'
)
version=${1:-$workspace_version}
if [[ ! "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  usage
fi
if [[ "$workspace_version" != "$version" ]]; then
  echo "workspace version is $workspace_version, expected $version" >&2
  exit 1
fi

crates=(rspyts-macros rspyts rspyts-cli)
publishable_crates=$(
  cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.publish != []) | .name' \
    | LC_ALL=C sort
)
if [[ "$publishable_crates" != $'rspyts\nrspyts-cli\nrspyts-macros' ]]; then
  echo "expected exactly the rspyts-macros, rspyts, and rspyts-cli publishable crates" >&2
  printf '%s\n' "$publishable_crates" >&2
  exit 1
fi
for crate in "${crates[@]}"; do
  crate_version=$(
    cargo metadata --no-deps --format-version 1 \
      | jq -r --arg crate "$crate" '.packages[] | select(.name == $crate) | .version'
  )
  if [[ "$crate_version" != "$version" ]]; then
    echo "$crate version is $crate_version, expected $version" >&2
    exit 1
  fi
done

if [[ "$allow_dirty" == false ]] && [[ -n $(git status --porcelain --untracked-files=normal) ]]; then
  echo "working tree must be clean; pass --allow-dirty only for local candidate checks" >&2
  exit 1
fi
package_args=(--locked --no-verify --allow-dirty)
cargo package "${package_args[@]}" \
  -p rspyts-macros \
  -p rspyts \
  -p rspyts-cli

verify_root=$(mktemp -d "${TMPDIR:-/tmp}/rspyts-crates.XXXXXX")
trap 'rm -rf "$verify_root"' EXIT
mkdir -p "$verify_root/.cargo"

for crate in "${crates[@]}"; do
  archive="target/package/${crate}-${version}.crate"
  if [[ ! -f "$archive" ]]; then
    echo "missing crate archive: $archive" >&2
    exit 1
  fi
  tar -xzf "$archive" -C "$verify_root"
done

cat >"$verify_root/.cargo/config.toml" <<EOF
[patch.crates-io]
rspyts-macros = { path = "rspyts-macros-${version}" }
rspyts = { path = "rspyts-${version}" }
EOF

for crate in "${crates[@]}"; do
  (
    cd "$verify_root"
    CARGO_TARGET_DIR="$repository_root/target/release-package-tests" \
      cargo test --manifest-path "${crate}-${version}/Cargo.toml"
  )
done
