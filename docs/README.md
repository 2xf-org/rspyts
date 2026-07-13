# Documentation

## Introduction

- [Quickstart](introduction/quickstart.md) — from `cargo new` to calling the same crate from Python and TypeScript.
- [How rspyts works](introduction/how-rspyts-works.md) — the C ABI, the envelope, code generation, and how this compares to PyO3, wasm-bindgen, and UniFFI.

## Guides

- [Python](python.md) — models, exceptions, numpy semantics, the GIL, packaging.
- [TypeScript](typescript.md) — instantiation, typed arrays, handle disposal, bundling.

## Project

- [Architecture](architecture.md) — how the crates, runtimes, and generated code fit together.
- [Releasing](releasing.md) — the lockstep release process across crates.io, PyPI, and npm.

## Design specs

These are normative. When a guide and a spec disagree, the spec wins.

- [ABI](design/abi.md) — the binary boundary.
- [Type system](design/type-system.md) — what can cross it, and why some things can't.
- [Codegen](design/codegen.md) — what gets generated, and the runtime API it may use.
- [Decisions](design/decisions.md) — the ADRs behind all of the above.
