# Releasing

rspyts releases are tag-driven. A stable tag publishes four Rust crates, the
Python runtime, and the npm runtime at one shared version.

## One-time registry setup

Configure trusted publishing for `.github/workflows/deploy.yml`:

- crates.io: the `crates-io` environment for `rspyts-core`, `rspyts-macros`,
  `rspyts`, and `rspyts-cli`;
- PyPI: the `pypi` environment for `rspyts`;
- npm: the `npm` environment for `rspyts`.

The workflow uses short-lived OIDC credentials. Do not add registry tokens to
repository secrets.

## Prepare a version

Update these sources together:

- `[workspace.package].version` and internal dependency versions in the root
  `Cargo.toml`;
- `runtimes/python/pyproject.toml` and
  `runtimes/python/src/rspyts/__init__.py`;
- `runtimes/typescript/package.json`.

Refresh the workspace and example lockfiles, then regenerate the examples:

```sh
cargo update --workspace
(cd runtimes/python && uv lock)
(cd runtimes/typescript && npm install --package-lock-only)

for directory in \
  examples/basic/python \
  examples/multi-crate/shared/python \
  examples/multi-crate/app/python; do
  (cd "$directory" && uv lock)
done

for directory in \
  examples/basic/typescript \
  examples/multi-crate/shared/typescript \
  examples/multi-crate/app/typescript; do
  (cd "$directory" && npm install --package-lock-only)
done

cargo run -p rspyts-cli -- generate --config examples/basic/rspyts.toml
cargo run -p rspyts-cli -- generate --config examples/multi-crate/shared/rspyts.toml
cargo run -p rspyts-cli -- generate --config examples/multi-crate/app/rspyts.toml
```

Review generated changes. A version header change is expected; unexplained API
changes are not.

## Run the release gate

The candidate commit must pass the validation workflow. A useful local subset
is:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo check --workspace --all-targets --locked

(cd runtimes/python && \
  uv sync --dev --locked --python 3.11 && \
  uv run ruff check . && \
  uv run ty check src && \
  uv run vulture && \
  uv run pytest)

(cd runtimes/typescript && \
  npm ci && \
  npm run build && \
  npm run typecheck && \
  npm run test:cov && \
  npm run check:surface)

cargo run -p rspyts-cli -- check --config examples/basic/rspyts.toml
cargo run -p rspyts-cli -- check --config examples/multi-crate/shared/rspyts.toml
cargo run -p rspyts-cli -- check --config examples/multi-crate/app/rspyts.toml
```

Build and smoke the exact registry candidates before tagging. `--allow-dirty`
is for a reviewed local tree; the deploy workflow omits it.

```sh
version=0.3.0
scripts/release/verify-rust-archives.sh --allow-dirty "$version"

rm -rf target/release-candidate
mkdir -p target/release-candidate/python target/release-candidate/npm
(cd runtimes/python && \
  uv build \
    --build-constraints ../../scripts/release/python-build-constraints.txt \
    --out-dir ../../target/release-candidate/python)
python3 scripts/release/check-python-sdist.py \
  target/release-candidate/python/rspyts-"$version".tar.gz \
  runtimes/python
scripts/release/smoke-python-distributions.sh \
  target/release-candidate/python "$version"

(cd runtimes/typescript && \
  npm ci && npm run build && \
  npm pack --pack-destination ../../target/release-candidate/npm)
scripts/release/smoke-npm-tarball.sh \
  target/release-candidate/npm "$version"
```

The Rust check unpacks and tests all four exact `.crate` archives together with
local dependency patches. This is needed before the new internal crate versions
exist on crates.io. The Python and npm checks install the exact wheel, sdist,
and tarball in isolated consumers and verify their package versions and public
and generator-facing import entrypoints. Runtime semantics are covered by the
normal test suites, not duplicated in release smokes.

## Tag and publish

After the candidate commit is on `main`, create and push an annotated, signed
stable tag:

```sh
git tag -s v0.3.0 -m "rspyts v0.3.0"
git push origin v0.3.0
```

The source guard rejects prerelease tags and verifies all of the following
before credentials or package publication are possible:

- the event is the exact tag push and `GITHUB_SHA` is its target commit;
- the tag is annotated and GitHub verifies its signature;
- the tag target is contained in `origin/main`;
- the complete reusable validation workflow passes at that immutable commit;
- the tag version matches the Cargo workspace version. Reusable validation
  separately checks that the Python and npm package versions match Cargo.

The deploy workflow then:

1. builds and tests the exact Rust package archives;
2. builds, checks, and installs the exact Python wheel and sdist;
3. builds, checks, and installs the exact npm tarball;
4. uploads the Python and npm candidates once as an immutable workflow
   artifact;
5. publishes Rust crates in dependency order: core, macros, facade, CLI;
6. downloads and publishes the preserved Python distributions;
7. downloads and publishes the preserved npm tarball with provenance;
8. creates a GitHub release with generated notes.

GitHub's artifact service records and verifies the upload digest, so the
workflow does not maintain a second checksum file. The workflow preserves the
tested `.crate` files so a rerun can compare their SHA-256 digests with crates.io.
Cargo still publishes from the same immutable commit: `cargo publish` performs
its own package build rather than accepting a prebuilt `.crate` upload, and the
workflow verifies that the resulting registry checksum matches the tested
candidate. Equivalent checksum checks protect PyPI and npm recovery as well.

Registry stages are sequential. The Rust stage waits for each dependency to
become visible before publishing the next. An existing version is accepted only
when its registry digest matches the tested candidate, so an interrupted
release is resumed by rerunning the same tag workflow. Never move a release tag
or reuse a registry version.

## Verify the release

After the workflow completes, inspect installed artifacts rather than the
working tree:

```sh
cargo install rspyts-cli --version 0.3.0 --locked
python -m venv /tmp/rspyts-release
/tmp/rspyts-release/bin/pip install rspyts==0.3.0
npm view rspyts@0.3.0 version
```

Confirm that each registry page renders its README, the GitHub release points
to the tagged commit, and published contents contain no repository-only files.
