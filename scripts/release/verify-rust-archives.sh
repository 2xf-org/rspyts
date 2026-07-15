#!/usr/bin/env bash

set -euo pipefail

usage() {
  echo "usage: $0 [--allow-dirty] <version>" >&2
  exit 2
}

allow_dirty=()
if [[ ${1:-} == "--allow-dirty" ]]; then
  allow_dirty=(--allow-dirty)
  shift
fi

[[ $# -eq 1 ]] || usage
version=$1
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || usage

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repository_root"

workspace_version=$(
  cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.name == "rspyts") | .version'
)
if [[ $workspace_version != "$version" ]]; then
  echo "workspace version is $workspace_version, expected $version" >&2
  exit 1
fi

crates=(rspyts-core rspyts-macros rspyts rspyts-cli)
cargo package \
  --locked \
  --no-verify \
  "${allow_dirty[@]}" \
  -p rspyts-core \
  -p rspyts-macros \
  -p rspyts \
  -p rspyts-cli

verify_root=$(mktemp -d "${TMPDIR:-/tmp}/rspyts-rust-archives.XXXXXX")
trap 'rm -rf "$verify_root"' EXIT
mkdir -p "$verify_root/.cargo"

for crate in "${crates[@]}"; do
  archive="target/package/${crate}-${version}.crate"
  [[ -f $archive ]] || {
    echo "missing Rust package archive: $archive" >&2
    exit 1
  }
  tar -xzf "$archive" -C "$verify_root"
done

cat >"$verify_root/.cargo/config.toml" <<EOF
[patch.crates-io]
rspyts-core = { path = "rspyts-core-${version}" }
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
