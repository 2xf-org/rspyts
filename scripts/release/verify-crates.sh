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
cargo package "${package_args[@]}" -p rspyts-macros
cargo package "${package_args[@]}" -p rspyts \
  --config "patch.crates-io.rspyts-macros.path='$repository_root/crates/rspyts-macros'"
cargo package "${package_args[@]}" -p rspyts-cli \
  --config "patch.crates-io.rspyts-macros.path='$repository_root/crates/rspyts-macros'" \
  --config "patch.crates-io.rspyts.path='$repository_root/crates/rspyts'"

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

require_exact_manifest_dependency() {
  local manifest=$1
  local package=$2
  local dependency=$3
  local expected=$4
  local requirements
  requirements=$(
    cd "$verify_root"
    cargo metadata --offline --no-deps --format-version 1 \
      --manifest-path "$manifest" \
      | jq -er --arg package "$package" --arg dependency "$dependency" '
          [
            .packages[]
            | select(.name == $package)
            | .dependencies[]
            | select(.name == $dependency)
            | .req
          ] as $requirements
          | if ($requirements | length) > 0
            then $requirements[]
            else error("missing dependency `" + $dependency + "`")
            end
        '
  )
  local requirement
  while IFS= read -r requirement; do
    if [[ "$requirement" != "=$expected" ]]; then
      echo "$package packaged dependency $dependency is $requirement, expected =$expected" >&2
      return 1
    fi
  done <<<"$requirements"
}

require_exact_packaged_dependency() {
  local crate=$1
  local dependency=$2
  local expected=$3
  require_exact_manifest_dependency \
    "$verify_root/${crate}-${version}/Cargo.toml" \
    "$crate" \
    "$dependency" \
    "$expected"
}

# Cargo publishes the normalized Cargo.toml, not the workspace-authored source
# manifest. Verify the exact requirements that prevent mutually generated Rust
# crates and wasm-bindgen tooling from resolving incompatible releases.
require_exact_packaged_dependency rspyts rspyts-macros "$version"
require_exact_packaged_dependency rspyts js-sys 0.3.103
require_exact_packaged_dependency rspyts wasm-bindgen 0.2.126
require_exact_packaged_dependency rspyts wasm-bindgen-test 0.3.76
require_exact_packaged_dependency rspyts-cli rspyts "$version"

# Prove the guard is semantic rather than a check that happens to accept the
# current archive. A bare Cargo version normalizes to a caret requirement and
# must be rejected without mutating any authored or packaged candidate file.
mutation="$verify_root/rspyts-non-exact-mutation"
cp -R "$verify_root/rspyts-${version}" "$mutation"
sed 's/version = "=0\.2\.126"/version = "0.2.126"/' \
  "$mutation/Cargo.toml" >"$mutation/Cargo.toml.mutated"
if cmp -s "$mutation/Cargo.toml" "$mutation/Cargo.toml.mutated"; then
  echo "failed to create non-exact packaged-manifest mutation" >&2
  exit 1
fi
mv "$mutation/Cargo.toml.mutated" "$mutation/Cargo.toml"
if require_exact_manifest_dependency \
  "$mutation/Cargo.toml" rspyts wasm-bindgen 0.2.126 \
  >/dev/null 2>&1; then
  echo "packaged-manifest verifier accepted a caret wasm-bindgen requirement" >&2
  exit 1
fi

"$repository_root/scripts/release/verify-private-data.sh" \
  "$verify_root/rspyts-macros-${version}" \
  "$verify_root/rspyts-${version}" \
  "$verify_root/rspyts-cli-${version}"

for crate in "${crates[@]}"; do
  (
    cd "$verify_root"
    CARGO_TARGET_DIR="$repository_root/target/release-package-tests" \
      cargo test --manifest-path "${crate}-${version}/Cargo.toml"
  )
done
