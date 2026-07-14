# Releasing

Releases are tag-driven and intentionally boring. A tag is published only
after the same commit passes the full validation workflow.

The repository publishes four Rust crates, one Python runtime, and one npm
runtime. All six packages use the same version.

## One-time registry setup

Configure trusted publishing before the first release:

- crates.io: register `.github/workflows/deploy.yml` and the `crates-io`
  environment for `rspyts-core`, `rspyts-macros`, `rspyts`, and `rspyts-cli`;
- PyPI: register the same workflow and the `pypi` environment for `rspyts`;
- npm: register the same workflow and the `npm` environment for `rspyts`.

The workflow uses short-lived OIDC credentials. Do not add long-lived registry
tokens to repository secrets.

## Prepare the version

Update these sources together:

- `[workspace.package].version` in the root `Cargo.toml`;
- internal Rust dependency versions in the workspace;
- `runtimes/python/pyproject.toml`;
- `runtimes/python/src/rspyts/__init__.py`;
- `runtimes/typescript/package.json`.

Then refresh lockfiles and generated clients:

```sh
cargo update --workspace
cd runtimes/python && uv lock
cd ../typescript && npm install --package-lock-only
cd ../..

cargo run -p rspyts-cli -- generate --config examples/basic/rspyts.toml
cargo run -p rspyts-cli -- generate --config examples/multi-crate/shared/rspyts.toml
cargo run -p rspyts-cli -- generate --config examples/multi-crate/app/rspyts.toml
```

Review generated changes. A new version header is expected; unexplained API
changes are not.

## Run the release gate

At minimum, the candidate must pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo check --workspace --all-targets --locked

cd runtimes/python
uv sync --dev --locked --python 3.11
uv run ruff check .
uv run ty check src
uv run vulture
uv run pytest
cd ../typescript
npm ci
npm run build
npm run typecheck
npm run test:cov
npm run check:surface
cd ../..

cargo run -p rspyts-cli -- check --config examples/basic/rspyts.toml
cargo run -p rspyts-cli -- check --config examples/multi-crate/shared/rspyts.toml
cargo run -p rspyts-cli -- check --config examples/multi-crate/app/rspyts.toml
```

Also build each registry artifact without publishing:

```sh
for crate in rspyts-core rspyts-macros rspyts rspyts-cli; do
  cargo package --list --locked --allow-dirty -p "$crate"
done

cd runtimes/python && uv build
cd ../typescript && npm pack --dry-run
```

The GitHub validation workflow repeats these checks across supported Python,
Node, Rust, macOS, Windows, native, and WebAssembly lanes.

## Tag and publish

Create a signed stable tag only after the candidate commit is on `main`:

```sh
git tag -s v0.2.0 -m "rspyts v0.2.0"
git push origin v0.2.0
```

`.github/workflows/deploy.yml` then:

1. runs the complete reusable validation workflow;
2. verifies that the tag, Cargo workspace, Python package, and npm package
   versions match;
3. builds the Python distributions and npm tarball once;
4. publishes Rust crates in dependency order: core, macros, facade, CLI;
5. publishes the preserved Python artifact to PyPI;
6. publishes the preserved npm artifact with provenance;
7. creates a GitHub release with generated release notes.

The registry stages are sequential. A later registry is never published if an
earlier one failed. Existing versions are detected so a failed workflow can be
rerun safely.

Manual workflow dispatch runs validation and version checks but cannot publish.

## Verify the release

After the workflow completes, inspect the installed artifacts rather than the
working tree:

```sh
cargo install rspyts-cli --version 0.2.0 --locked
python -m venv /tmp/rspyts-release
/tmp/rspyts-release/bin/pip install rspyts==0.2.0
npm view rspyts@0.2.0 version
```

Confirm that every registry page renders its package README, the GitHub release
points to the tagged commit, and the published package contents contain no
repository-only material.

If publication fails, fix the cause and rerun the same workflow. Never move or
replace a published tag, and never reuse a registry version.
