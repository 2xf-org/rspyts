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

## Trusted publishing and one-time setup

All registries publish with GitHub Actions OIDC. The repository stores no long-lived registry credentials. Each publishing job uses a matching GitHub environment and receives a short-lived token for that run.

### crates.io Trusted Publishing (no token)

For each of `rspyts-core`, `rspyts-macros`, `rspyts`, and `rspyts-cli`, configure a GitHub Trusted Publisher on crates.io with owner `2xf-org`, repository `rspyts`, workflow `deploy.yml`, and environment `crates-io`. Create the matching `crates-io` environment in the GitHub repository. The workflow's `rust-lang/crates-io-auth-action` step exchanges its GitHub OIDC identity for a short-lived Cargo registry token.

### PyPI Trusted Publishing (no token)

PyPI publishes via OIDC — there is no `PYPI_TOKEN`. One-time setup:

1. On pypi.org, go to the `rspyts` project → **Manage** → **Publishing** → **Add a new publisher** → GitHub.
2. Enter: owner `2xf-org`, repository `rspyts`, workflow name `deploy.yml`, environment `pypi`.
3. On GitHub, create the matching environment: repo Settings → **Environments** → New environment → `pypi`. Optionally require a reviewer for this environment — that makes PyPI publishes a manual approval step.

For the **first** release, the project does not exist on PyPI yet: use a *pending* publisher instead (pypi.org → your account → **Publishing** → "Add a new pending publisher") with the same four values plus the project name `rspyts`. The first successful workflow run creates and claims the project.

### npm Trusted Publishing (no token)

On npm, open the `rspyts` package settings and configure a GitHub Actions Trusted Publisher with owner `2xf-org`, repository `rspyts`, workflow `deploy.yml`, environment `npm`, and the `npm publish` action allowed. Create the matching `npm` environment in the GitHub repository. The workflow uses Node 24/npm 11 and `id-token: write`, so npm authenticates the publish through OIDC and emits provenance without an `NPM_TOKEN`.

## First-release checklist

- crates.io and npm require a one-time bootstrap publish before Trusted Publishing can be attached to a new package. Use narrowly scoped, short-lived tokens for that bootstrap only, configure the Trusted Publishers immediately afterward, then revoke and delete the tokens.
- crates.io has no name reservation — bootstrap-publish `rspyts-core` → `rspyts-macros` → `rspyts` → `rspyts-cli` in order, same as always. Add co-owners afterward with `cargo owner --add` if needed.
- npm: the bootstrap publish of `rspyts` claims the package name for the publishing account; grant the org team access afterward if needed.
- PyPI: the pending publisher (above) claims the project name on first publish.

## After the release

- Verify the three registries show the new version and that `cargo install rspyts-cli`, `pip install rspyts`, and `npm install rspyts` resolve to it.
