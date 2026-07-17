# Maintaining rspyts

rspyts publishes `rspyts-macros`, `rspyts`, and `rspyts-cli` from one Cargo
workspace. All three crates always have the same version.

## Product boundary

Keep the supported surface small:

- one workspace pinned to Rust and Cargo 1.88.0;
- one contract module per crate;
- one exact rspyts version throughout a contract graph;
- generated Python source for a Maturin abi3 wheel;
- either static TypeScript or browser WASM with a canonical `./wire` export;
- at most one direct dependency, which must be a leaf; and
- fixed ignored `.rspyts/` output with a committed semantic `rspyts.lock`.

Reject unsupported input instead of inventing another build mode. Update the
semantic IR, validation, emitters, macros, fixtures, and documentation together.
Generated npm packages must retain fingerprint checks as import-time side
effects.

## Validation

The complete gate is [the validation workflow](.github/workflows/validation.yml).
It covers Rust formatting, linting, tests, isolated target features, installed
Python wheels, packed browser/WASM packages, static TypeScript, direct-owner
identity and stale-lock rejection, supported operating systems, rustdoc, local
documentation links, privacy scans, and exact crate archives.

Compiler, macro, emitter, discovery ABI, lock, or packaging changes require the
complete workflow. Test built and installed consumer artifacts, not only files
inside `.rspyts/`.

## Prepare a release

Use the repository toolchain from `rust-toolchain.toml`. From a clean checkout,
build and test the exact Cargo archives:

```sh
version=0.4.2
scripts/release/verify-crates.sh "$version"
```

The script requires exactly the three expected publishable crates at one
version. It creates their archives, unpacks them, redirects their rspyts
dependencies to those unpacked sources, and tests the result. During local
development only, `--allow-dirty` can inspect unfinished work; it is not a
release procedure.

Before releasing a new version:

1. Update the workspace version and exact internal dependency pins.
2. Regenerate `Cargo.lock`.
3. Update exact-version install examples and the changelog.
4. Run the complete validation workflow and archive verification.
5. Merge the release commit to `main`.

## Publish

From a clean checkout exactly matching `origin/main`, push one annotated stable
version tag:

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

[The deploy workflow](.github/workflows/deploy.yml) verifies that the tag is
annotated and contained in `main`, reruns validation, preserves exact archives
and checksums, publishes the crates in dependency order with trusted
publishing, installs the published CLI, compiles a clean consumer, and creates
the GitHub release.

The three crates.io trusted publishers must name organization `2xf-org`,
repository `rspyts`, workflow `deploy.yml`, and environment `crates-io`. Normal
releases use GitHub OIDC and need neither `cargo login` nor a long-lived token.

The `RELEASE_PRIVATE_PATTERNS_B64` repository secret contains a base64-encoded,
newline-delimited set of protected extended regular expressions. Candidate
archives and release notes fail closed if that list is unavailable, empty,
invalid, or matched. Keep the plaintext patterns outside the public repository.

Do not publish manually, move or reuse a release tag, or overwrite a published
version. If a published checksum differs from the preserved candidate, stop and
release a corrected patch version.
