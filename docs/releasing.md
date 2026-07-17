# Releasing

rspyts 0.4 publishes three Cargo crates and one GitHub release. It publishes no
PyPI or npm rspyts package.

## Prerequisites

- Release from a clean `main` commit already pushed to `origin/main`.
- Keep the trusted-publishing workflow at `.github/workflows/deploy.yml` unless
  the crates.io publisher configuration is deliberately updated.
- Use the GitHub `crates-io` environment with OIDC (`id-token: write`).
- Never use a long-lived registry token for the normal release.
- The reusable [validation workflow](../.github/workflows/validation.yml) must
  be green.
- All [delivery gates](design/v0.4-delivery.md) must pass against exact local
  candidates, including downstream consumer acceptance.

Trusted publishing is a one-time crates.io setting for each of
`rspyts-macros`, `rspyts`, and `rspyts-cli`. In each crate's settings, configure
the GitHub publisher as:

| Field | Value |
| --- | --- |
| Organization | `2xf-org` |
| Repository | `rspyts` |
| Workflow | `deploy.yml` |
| Environment | `crates-io` |

The workflow and crates.io setting must match exactly. The action exchanges a
GitHub OIDC identity for a short-lived token and revokes it after the job; no
GitHub secret, personal crates.io API key, or `cargo login` is part of the
normal release. See the official
[crates.io trusted-publishing guide](https://crates.io/docs/trusted-publishing).

## Candidate checks

Before creating a tag:

1. Verify every workspace package reports the intended version.
2. Run the full validation matrix: format, Clippy, tests, MSRV, operating
   systems, target features, generated Python wheel, browser/WASM package,
   static TypeScript package, docs, links, and package archives.
3. Run `scripts/release/verify-crates.sh VERSION` to package all three crates.
4. Let that script unpack and test the exact `.crate` archives in dependency
   order.
5. Exercise consumer wheel and browser artifacts built with those candidates.
6. Confirm generated output is untracked and consumer artifacts have no
   Python/npm rspyts runtime dependency.
7. Confirm `main`, documentation, and lockfiles contain the same release
   version.

Candidate artifacts can be tested before publication; registry artifacts
cannot. Do not describe a pre-tag check as a published-artifact smoke test.

## Tag and publish

Create one annotated immutable tag:

```sh
git fetch origin
git switch main
git pull --ff-only origin main
test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
test -z "$(git status --porcelain)"
git tag -a v0.4.1 -m "rspyts v0.4.1"
git push origin refs/tags/v0.4.1
```

The tag-driven [deployment workflow](../.github/workflows/deploy.yml) reruns
validation, preserves the exact archives and SHA-256 checksums, then publishes
sequentially:

1. `rspyts-macros`;
2. `rspyts`;
3. `rspyts-cli`;
4. clean crates.io smoke: install the exact CLI and compile a fresh consumer
   using `rspyts` and its derive macros;
5. GitHub release with the preserved archives and checksums.

Do not run `cargo publish` manually. Do not create a PyPI or npm release job.

## Verify the registries

The workflow performs a clean crates.io install and compile smoke before it
creates the GitHub release. After the workflow succeeds, repeat the CLI install
locally when performing downstream acceptance:

```sh
cargo install rspyts-cli --version =0.4.1 --locked --force
```

Create the quickstart contract, run `build`, `lock`, and `check --locked`, then
rebuild downstream consumers using registry dependencies instead of local
paths. Compare their semantic fingerprints with the accepted candidates.

## Partial failure

Rerun failed jobs on the same tag. The workflow must treat an already-published
crate as complete only when its registry checksum matches the candidate.

Never move or reuse a release tag, overwrite a published version, or continue
after a checksum mismatch. Fix the cause and release the next patch version.
