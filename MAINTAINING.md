# Maintaining rspyts

rspyts publishes three Cargo crates from one versioned workspace:
`rspyts-macros`, `rspyts`, and `rspyts-cli`. It does not publish to PyPI or npm.

## Development contract

- Keep the 0.4 contract language deliberately closed. Reject unsupported Rust
  instead of guessing a host shape or adding consumer-authored bridge models.
- Change the semantic IR, validation, emitters, macros, fixtures, and docs
  together.
- Keep generated `.rspyts/` output untracked. Commit example and consumer
  `rspyts.lock` files when their semantic API changes.
- Preserve Rust 1.88 compatibility.
- Test built consumer artifacts, not only generated source directories.

## Local validation

The fast workspace gate is:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked
```

Do not maintain a second hand-copied acceptance script. The complete,
authoritative commands are the jobs in
[`validation.yml`](.github/workflows/validation.yml): isolated Python and WASM
features, an installed Python 3.11 wheel, a packed browser/WASM package in
Chromium, static TypeScript, cross-package identity and stale-lock rejection,
Rust 1.88, Linux/macOS/Windows, rustdoc, link checking, and exact crate
archives. Compiler, macro, emitter, or packaging changes require that complete
workflow, not only the fast gate above.

## Testing a release candidate

Use Rust/Cargo 1.94 for reproducible package candidates and install `jq`:

```sh
version=0.4.2
scripts/release/verify-crates.sh "$version"
```

The script requires a clean worktree, confirms that exactly the three expected
crates are publishable at one version, creates their `.crate` archives, unpacks
them, patches dependencies to the unpacked sources, and tests those exact
sources. During an unfinished local change only, use:

```sh
scripts/release/verify-crates.sh --allow-dirty "$version"
```

`--allow-dirty` is not a release procedure. A candidate is not a published
artifact until the registry workflow succeeds.

## Releasing

1. Choose a stable `MAJOR.MINOR.PATCH` version.
2. Update the workspace version in `Cargo.toml`, regenerate `Cargo.lock`, and
   update exact-version install examples and versioned documentation together.
3. Run the full validation workflow and exact candidate script.
4. Merge the release commit to `main`; releases cannot originate from an
   unmerged branch.
5. From a clean checkout exactly matching `origin/main`, push one annotated
   tag:

```sh
git fetch origin
git switch main
git pull --ff-only origin main
test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
test -z "$(git status --porcelain)"
version=0.4.2
git tag -a "v$version" -m "rspyts v$version"
git push origin "refs/tags/v$version"
```

The tag starts [`.github/workflows/deploy.yml`](.github/workflows/deploy.yml).
It:

1. proves the tag is annotated, stable, and contained in `origin/main`;
2. reruns the complete validation workflow;
3. builds and preserves the three exact archives plus `SHA256SUMS`;
4. publishes `rspyts-macros`, `rspyts`, then `rspyts-cli` through crates.io
   trusted publishing;
5. installs the exact CLI from crates.io and compiles a fresh consumer;
6. creates or verifies the GitHub release and its preserved assets.

The crates.io publishers for all three crates must name organization `2xf-org`,
repository `rspyts`, workflow `deploy.yml`, and environment `crates-io`. The
workflow uses GitHub OIDC; normal releases need no long-lived crates.io token
and no `cargo login`.

Do not run `cargo publish` manually, publish rspyts to PyPI/npm, move or reuse a
release tag, or overwrite a published version.

## Failure handling

Rerun failed jobs against the same immutable tag. The deploy workflow treats an
already-published crate as complete only when its crates.io checksum matches
the preserved candidate.

A checksum mismatch is terminal for that version. Do not continue publishing
and do not replace the tag; fix the cause and release a new patch version.
