# Releasing

Every release ships the four crates (crates.io), the Python runtime (PyPI), and the TypeScript runtime (npm) in lockstep, with the same version number. A runtime speaks exactly one ABI major version, and aligned package versions are what let users reason about compatibility — everything at 0.3.x works together. Do not release one surface without the others.

## 1. Bump versions

All in one commit:

1. `Cargo.toml` (workspace root): `[workspace.package] version` **and** the three path-dependency pins in `[workspace.dependencies]` (`rspyts-core`, `rspyts-macros`, `rspyts`) — cargo publishes these as real version requirements.
2. `runtimes/python/pyproject.toml`: `version`.
3. `runtimes/typescript/package.json`: `version`.
4. `runtimes/python/src/rspyts/__init__.py`: `__version__`.
5. Run `cargo build` to refresh `Cargo.lock`, `uv lock --directory runtimes/python`, and `npm install --package-lock-only --prefix runtimes/typescript`. Then regenerate the example (`cargo run -p rspyts-cli -- generate --config examples/basic/rspyts.toml`) so the generated-file headers carry the new version. Commit everything.

The deploy workflow rejects tags that do not exactly match every package and lockfile version. Releases currently use stable `vX.Y.Z` tags; add explicit cross-ecosystem prerelease normalization before introducing prerelease tags.

Note that the ABI version (`ABI_VERSION` in `crates/rspyts-core/src/lib.rs`) is **not** bumped on release — only when the boundary itself changes. Any change to [abi.md](design/abi.md)'s contract must bump it and update the shims, the Python runtime, and the TypeScript runtime in the same PR; runtimes reject modules with an unknown ABI major version, so a partial update fails loudly at load time.

## 2. Tag

```
git tag vX.Y.Z
git push origin vX.Y.Z
```

Pushing a `v{X.Y.Z}` tag triggers `.github/workflows/deploy.yml`. Its `verify` job runs the Rust workspace tests, then the publish jobs push to all three registries; the runtime and example suites already ran on `main` via `validation.yml`.

## 3. Publish order (crates.io)

Dependency order, each crate only after its dependencies are visible on the registry:

1. `rspyts-core`
2. `rspyts-macros`
3. `rspyts`
4. `rspyts-cli`

The workflow does this sequentially with a registry availability check between publishes (crates.io indexing is eventually consistent). PyPI and npm have no ordering constraint and publish in parallel with the crates.io chain.

## Required secrets and one-time setup

The release workflow needs the following configured on the `2xf-org/rspyts` repository.

### `CARGO_REGISTRY_TOKEN` (repository secret)

A crates.io API token with the `publish-new` + `publish-update` scopes (crates.io → Account Settings → API Tokens → New Token; restrict to the four crate names). Add it under Settings → Secrets and variables → Actions → New repository secret.

### PyPI Trusted Publishing (no token)

PyPI publishes via OIDC — there is no `PYPI_TOKEN`. One-time setup:

1. On pypi.org, go to the `rspyts` project → **Manage** → **Publishing** → **Add a new publisher** → GitHub.
2. Enter: owner `2xf-org`, repository `rspyts`, workflow name `deploy.yml`, environment `pypi`.
3. On GitHub, create the matching environment: repo Settings → **Environments** → New environment → `pypi`. Optionally require a reviewer for this environment — that makes PyPI publishes a manual approval step.

For the **first** release, the project does not exist on PyPI yet: use a *pending* publisher instead (pypi.org → your account → **Publishing** → "Add a new pending publisher") with the same four values plus the project name `rspyts`. The first successful workflow run creates and claims the project.

### `NPM_TOKEN` (repository secret)

The first publish needs an npm **granular access token** (npmjs.com → Access Tokens → Generate New Token → Granular), permissions **Read and write**, with package creation/publish allowed. Add it as repository secret `NPM_TOKEN`. The package cannot be scoped to `rspyts` until that package exists.

After the first release, configure npm Trusted Publishing on the `rspyts` package for owner `2xf-org`, repository `rspyts`, workflow `deploy.yml`, then remove `NODE_AUTH_TOKEN` from the workflow and delete `NPM_TOKEN`. The npm publish job already grants `id-token: write` and uses a compatible npm/Node version.

## First-release checklist

- crates.io names are claimed by the token owner. The first `cargo publish` of each crate registers the name to whoever owns `CARGO_REGISTRY_TOKEN`; make sure that account is the org owner, and add co-owners afterwards with `cargo owner --add`. There is no name reservation — publish `rspyts-core` → `rspyts-macros` → `rspyts` → `rspyts-cli` in order, same as always.
- npm: the first publish of `rspyts` claims the package name for the token owner; grant the org team access afterwards.
- PyPI: the pending publisher (above) claims the project name on first publish.

## After the release

- Verify the three registries show the new version and that `cargo install rspyts-cli`, `pip install rspyts`, and `npm install rspyts` resolve to it.
