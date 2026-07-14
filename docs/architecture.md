# Architecture

How the pieces of rspyts fit together. The normative contracts live in [abi.md](design/abi.md), [type-system.md](design/type-system.md), and [codegen.md](design/codegen.md); this document is the map.

## Components

```
                        ┌────────────────────┐
                        │   rspyts-macros    │  #[bridge] expansion:
                        │   (proc macros)    │  shims + registrations
                        └─────────┬──────────┘
                                  │ generated code calls into
┌────────────────────┐  ┌─────────▼──────────┐  ┌────────────────────┐
│    rspyts-cli      │  │    rspyts-core     │  │   rspyts (facade)  │
│ generate/check/init│──│ IR · envelope ·    │──│ public surface +   │
│ dlopen + emitters  │  │ handles · shims    │  │ export!() macro    │
└─────────┬──────────┘  └────────────────────┘  └────────────────────┘
          │ emits
          ▼
┌──────────────────────────────┐     ┌───────────────────────────────┐
│  generated code (yours)      │────▶│  runtimes (published)         │
│  Python package · TS files   │     │  runtimes/python  → PyPI      │
│  schema.json                 │     │  runtimes/typescript → npm    │
└──────────────────────────────┘     └───────────────────────────────┘
```

- **`crates/rspyts-core`** — everything shared between the macros, the generated shims, and the CLI: the manifest IR, the response envelope binary format, the `BridgeError` model, the handle slab, the panic-safe shim entry points, and the inventory-based registry. Semver-exempt plumbing; application code never depends on it directly.
- **`crates/rspyts-macros`** — the `#[bridge]` attribute. Each expansion produces serde derives and a `Bridged` impl for data types, an `extern "C"` shim per function/method that delegates entirely to `rspyts_core::shim`, and an `inventory::submit!` record whose builder returns the item's IR declaration.
- **`crates/rspyts`** — the facade. Re-exports the public names (`bridge`, `Buf`, `BridgeError`, `BridgeErr`, `Bridged`), defines `export!()` (the four module-level symbols), and hides macro plumbing under `#[doc(hidden)] __private`.
- **`crates/rspyts-cli`** — the `rspyts` binary. Builds the target crate, `dlopen`s the resulting cdylib, pulls the manifest out of the compiled module, and runs the emitters. `check` renders the same outputs in memory and diffs against disk — the CI drift gate.
- **`runtimes/python`** — PyPI package `rspyts`: `Contract` (pydantic base), the error hierarchy, and `Library` (ctypes loader/caller). Pure Python; numpy and pydantic are hard dependencies.
- **`runtimes/typescript`** — npm package `rspyts`: `instantiate`, `callFn`/`callDrop`, `SliceArg`, and the error classes for the WASM path.
- **Generated code** — written into directories the CLI wholly owns, configured in `rspyts.toml`. It may import only the runtime APIs frozen in [codegen.md §4.1/§5.1](design/codegen.md); everything else it needs is baked in at generation time.

## Anatomy of a call

What happens when Python calls a generated wrapper (native cdylib path):

1. Application code calls `summarize(values, label)` in the generated `functions.py`.
2. The wrapper splits arguments per [ABI §3.1](design/abi.md): plain parameters become one wire-cased JSON object (models dumped `by_alias`); slice parameters are passed separately as `(array, dtype)` pairs.
3. `Library.call` lazily loads the cdylib on first use — resolution order: `RSPYTS_LIBRARY` env var, then an explicit path override, then the baked-in search list — and verifies `rspyts_abi_version() == 1`.
4. `Library.call` makes each slice C-contiguous with the right dtype (zero-copy if it already is), writes the args JSON into a buffer, and invokes `rspyts_fn__summarize(args_ptr, args_len, s0_ptr, s0_len)` through ctypes — which releases the GIL for the duration.
5. Inside Rust, the macro-generated shim runs entirely under `catch_unwind`: deserialize the args, reconstitute the borrowed slices, call the user function, map its `Result` through `BridgeErr`.
6. The outcome is encoded into a single envelope allocation ([ABI §4](design/abi.md)): status byte, JSON payload, and a raw tail. Any `Buf<T>` in the return value serializes as a `{"__rspyts_buf__": {off, len, dt}}` placeholder while its bytes are appended, aligned, to the tail.
7. Back in Python, `Library.call` reads the 12-byte header, parses the JSON, and replaces every placeholder (at any depth) with a numpy array **copied** out of the tail. It then frees the envelope and its own args buffer via `rspyts_free` — the caller frees everything it allocated plus everything it received.
8. Status 0: the wrapper validates the payload into the return model and hands it back. Status 1: the code→class registry raises the generated exception subclass. Status 2: `RspytsPanicError`.

The TypeScript/WASM path is step-for-step identical, with mechanical substitutions: ctypes → WASM exports, args copied into linear memory via `rspyts_alloc`, numpy → typed arrays (also always copied out, because linear memory may grow and move), handles as `bigint` instead of `int`, and no GIL to release. Method calls differ from free functions only by a leading `u64` handle argument; constructors return a handle in the envelope JSON; `__drop` returns nothing and is idempotent.

## Anatomy of `rspyts generate`

1. Each `#[bridge]` expansion registered a builder with `inventory` at compile time. `export!()` defined `rspyts_manifest()`, which collects all records, sorts each section by name, panics on duplicates, and encodes the manifest as an ordinary envelope.
2. The CLI runs `cargo build --message-format=json` to locate the cdylib artifact robustly, `dlopen`s it, checks `rspyts_abi_version() == 1`, calls `rspyts_manifest()`, and deserializes the IR. Rust source is never parsed.
3. Every enabled emitter (Python, TypeScript, JSON Schema) renders from the manifest alone, under the determinism rules of [codegen.md §3](design/codegen.md): manifest order only, no timestamps, a `DO NOT EDIT` header plus the manifest SHA-256 in every file.
4. Outputs are written only when content changed (stable mtimes); `rspyts check` diffs instead of writing and exits 1 on drift.

## Invariants

Things that are always true, and where they are enforced:

- **Panics never cross the boundary.** Every exported shim body runs inside `catch_unwind` (`rspyts-core/src/shim.rs`); panics become status-2 envelopes. On `wasm32-unknown-unknown` (panic = abort) a panic traps the instance instead — same shim, less forgiving platform.
- **The caller frees.** Request buffers are allocated, owned, and freed by the caller; envelopes are returned to the caller, who frees them with `rspyts_free(ptr, 12 + json_len + tail_len)`. Rust never frees foreign requests; runtimes always free envelopes, even on error paths.
- **Envelope capacity == length** (`envelope::seal`), so a single `(ptr, total_len)` pair frees the allocation exactly.
- **Manifests and generated files are deterministic.** Sections sorted and uniqueness-asserted in `registry::build_manifest`; emitters iterate manifest order only. Same crate in, byte-identical files out — this is what makes committed generated code and the drift check work.
- **Handles are never 0, never reused,** and enforced below 2^53 (`handles::Slab`: per-type monotonic counter from 1; `insert` refuses at `MAX_HANDLE`), so they survive JSON and JS `number` transport. `__drop` is idempotent; stale handles produce a `staleHandle` error, not UB.
- **Unknown fields are rejected in both directions** (serde `deny_unknown_fields`, pydantic `extra="forbid"`). Wire compatibility is explicit, never accidental.
- **Unbridgeable types fail at compile time.** `u64`, `i64`, tuples, and friends simply don't implement `Bridged` (`rspyts-core/src/bridged.rs`).

## Where to look

| Path | What's there |
|---|---|
| `crates/rspyts-core/src/ir.rs` | The manifest IR — authoritative serde shapes for [ABI §7](design/abi.md) |
| `crates/rspyts-core/src/envelope.rs` | Envelope encode/decode, `alloc`/`dealloc`, the tail collector for `Buf` |
| `crates/rspyts-core/src/error.rs` | `BridgeError`, well-known codes, the `BridgeErr` trait |
| `crates/rspyts-core/src/bridged.rs` | The `Bridged` trait, `Buf<T>`, the `__rspyts_buf__` placeholder |
| `crates/rspyts-core/src/registry.rs` | Inventory records, `build_manifest` (sorting + uniqueness) |
| `crates/rspyts-core/src/handles.rs` | The `Slab` behind opaque class handles |
| `crates/rspyts-core/src/shim.rs` | `run`, `run_drop`, `decode_args`, `slice_arg`, `map_result`/`map_plain` |
| `crates/rspyts-macros/src/lib.rs` | `#[bridge]` parsing and expansion |
| `crates/rspyts/src/lib.rs` | Facade re-exports, `export!()`, `__private` |
| `crates/rspyts-cli/src/` | Config parsing, build + dlopen, the three emitters, `check` diffing |
| `runtimes/python/` | `Contract`, `Library`, error hierarchy (PyPI `rspyts`) |
| `runtimes/typescript/` | `instantiate`, `callFn`, error classes (npm `rspyts`) |
| `examples/basic/` | The end-to-end example; drift-gated in CI |
