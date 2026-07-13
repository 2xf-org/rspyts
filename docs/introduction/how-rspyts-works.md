# How rspyts works

rspyts makes one Rust crate the single source of truth for a cross-language contract. You annotate types, constants, functions, and impl blocks with `#[bridge]`, and `rspyts generate` projects them into pydantic v2 models, TypeScript types, JSON Schema, importable constants, and fully typed wrapper functions for both languages.

There is no deep per-language integration. Everything crosses one small, hand-specified C ABI — the same boundary whether the crate is compiled as a native cdylib (Python, via `ctypes`) or as a WebAssembly module (TypeScript).

```
        ┌───────────────────────────────────┐
        │          your Rust crate          │
        │   #[bridge] types · fns · impls   │
        │         rspyts::export!()         │
        └────────┬─────────────────┬────────┘
                 │                 │
        cargo build         rspyts generate
     (cdylib / wasm32)   (dlopen → manifest → emitters)
                 │                 │
                 ▼                 ▼
   ┌──────────────────────┐   ┌───────────────────────────────┐
   │      tiny C ABI      │   │        generated code         │
   │  4 module symbols +  │   │  pydantic models · TS types   │
   │  one per fn/method   │   │  JSON Schema · typed wrappers │
   │  JSON envelopes ·    │   └───────────────────────────────┘
   │  raw numeric buffers │
   └────┬────────────┬────┘
        │ ctypes     │ WebAssembly
        ▼            ▼
     Python      TypeScript
  (pip rspyts)  (npm rspyts)
```

## The boundary

Only three kinds of things ever cross it:

- **Structured data** crosses as UTF-8 JSON inside a response envelope with a 12-byte header. Structs, enums, `Option`, `Vec`, maps — all serialized, all copied, no shared identity.
- **Bulk numeric data** crosses as raw pointer+length buffers: `&[T]` parameters in, `Buf<T>` returns out. numpy arrays and JS typed arrays, no JSON in the hot path.
- **Stateful objects** never cross at all. Their state stays in Rust behind a `u64` handle; Python and TypeScript hold a wrapper class around an integer.

Panics never unwind across the boundary — every exported shim runs under `catch_unwind` and a panic becomes a typed error envelope. Unknown fields are rejected on both sides. Anything the boundary cannot carry (`u64`, tuples, generics) is a compile-time error at the Rust definition site.

The full contract fits in three documents: [ABI](../design/abi.md), [type system](../design/type-system.md), [codegen](../design/codegen.md).

## Code generation

`#[bridge]` does two things at compile time. It generates an `extern "C"` shim for every function, method, and static, and it registers a description of the item — its name, fields, types, doc comments, and for constants the serialized value — into the crate itself. `rspyts::export!()` exposes that description as a manifest the compiled module can hand back.

`rspyts generate` then builds the crate, `dlopen`s the resulting cdylib, and asks it for its manifest. The CLI never parses Rust source — the compiled module is the authority on its own contract. Each emitter (Python, TypeScript, JSON Schema) renders from the manifest alone, deterministically: same crate in, byte-identical files out.

That determinism is what makes the recommended workflow possible: commit the generated code, and let `rspyts check` fail CI with a unified diff whenever it drifts from the Rust definitions. A contract change is always a reviewable diff, in all four languages at once.

## Questions & Answers

**How does this differ from** `PyO3` **and** `wasm-bindgen`**?** Purpose and depth. PyO3 binds into CPython's object model and wasm-bindgen binds into the JS host — both are excellent, and both are large (roughly 69,000 and 40,000 lines of Rust). Adopting both means two big binding layers with independent semantics for ownership, error mapping, and GC interaction. rspyts needs less: move typed data, call functions, hold opaque state. So it crosses one small C ABI and keeps both runtimes thin. The cost is real: every call pays serialization, and there is no fine-grained object integration — you cannot subclass a Rust type in Python. If you need that, PyO3 is the right tool.

**How does this differ from** `UniFFI`**?** Mostly in targets and inputs. UniFFI (Mozilla) is the same architectural idea — serialize every call over a hand-written C ABI — aimed at Kotlin, Swift, and Python, driven by its own scaffolding. rspyts aims at exactly Python and TypeScript, generates pydantic v2 and strict TS surfaces plus JSON Schema, and reads its contract out of the compiled module rather than from an interface file.

**Why JSON on the wire?** Because `JSON.parse` and pydantic-core are privileged fast paths on both hosts, and a payload you can print is a payload you can debug. Bulk numeric data never touches JSON anyway — the raw tail carries it. A binary codec remains a non-breaking future addition ([ADR-2](../design/decisions.md#adr-2-json-as-the-v01-wire-format)).

**Why are** `u64` **and** `i64` **rejected?** JavaScript numbers are IEEE-754 doubles; integers above 2^53 silently lose precision, and JSON inherits the same ceiling. rspyts refuses at compile time rather than corrupt silently, stringify surprisingly, or infect every TS call site with `BigInt` ([ADR-4](../design/decisions.md#adr-4-u64i64-rejected-at-compile-time)).

Every decision above has a full record — context, alternatives, costs — in [decisions.md](../design/decisions.md).
