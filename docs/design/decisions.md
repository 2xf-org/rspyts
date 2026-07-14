# Design decisions

Architecture Decision Records for rspyts. Each records the context at
decision time, the decision, and its consequences — including the costs.
The specs these decisions produced are [abi.md](abi.md),
[type-system.md](type-system.md), and [codegen.md](codegen.md); when a
guide or README says "deliberately not supported", this file is the
receipt. All records: **Accepted, v0.1**.

## ADR-1: A message-passing C ABI, not PyO3 + wasm-bindgen deep integration

**Context.** The obvious way to build "Rust callable from Python and
TypeScript" is the two flagship integrations: PyO3 binding into CPython's
object model, and wasm-bindgen binding into the JS host. Both are
excellent, and both are *deep*: PyO3 is roughly 69,000 lines of Rust,
wasm-bindgen roughly 40,000. Adopting both means two large binding layers
with independent semantics (ownership, error mapping, GC interaction),
compiled differently per target, and a boundary that can only be audited by
reading both codebases. rspyts's actual need is narrower: move typed data,
call functions, hold opaque state.

**Decision.** Cross one tiny, hand-specified C ABI ([abi.md](abi.md)):
everything on the boundary is serialized bytes, raw numeric buffers, or
opaque `u64` handles — nothing else, ever. Both runtimes are thin
consumers: Python uses stock `ctypes`, TypeScript uses plain WASM exports.
This is a well-trodden road: Mozilla's UniFFI serializes every call over a
hand-written C ABI, Google's Diplomat generates bindings over a restricted
C boundary, and the WASM Component Model's Canonical ABI is the same idea
standardized — lift and lower values across a narrow waist instead of
integrating object models.

**Consequences.** One boundary to specify, audit, fuzz, and test; native
and WASM behave identically by construction; a new target language costs a
small runtime, not a binding framework. The costs: every call pays
serialization (bounded by keeping bulk data out of JSON — ABI §6), there is
no fine-grained object integration (you cannot subclass a Rust type in
Python), and every future capability (callbacks, async) must be designed
into the ABI explicitly rather than inherited from a framework.

## ADR-2: JSON as the v0.1 wire format

**Context.** The envelope needs an encoding for structured payloads.
Binary candidates (CBOR, postcard, bincode) are smaller and faster to
encode; JSON is universally debuggable and has privileged, heavily
optimized decoders on both target platforms.

**Decision.** UTF-8 JSON in v0.1. `pydantic` and `JSON.parse` are native
fast paths — for typical payload sizes, `JSON.parse` beats JS-level binary
decoding, and pydantic-core validates JSON directly in Rust. A payload you
can print is a payload you can debug with nothing but `console.log`. Bulk
numeric data never touches JSON anyway (the raw tail, ABI §6), so the
classic "JSON is slow for arrays" cost does not apply where it would hurt.

**Consequences.** The wire is inspectable end to end, and the JSON Schema
output describes the actual bytes. A binary codec remains a non-breaking
future addition: the envelope frames its payload as opaque bytes with
explicit lengths, so a negotiated codec (CBOR or postcard) changes the
payload encoding without touching the envelope layout, the symbols, or the
generated API surfaces. Cost: verbose encoding for deeply nested small
messages, and JSON's effective 2^53 integer ceiling — which the type
system enforces regardless (ADR-4).

## ADR-3: Commit generated code, gate it with a drift check

**Context.** Generated code either materializes invisibly at build time or
is committed to the repository. The invisible route has a documented
failure mode: Prisma spent years generating its client into
`node_modules/.prisma` and walked it back — code that isn't in the repo
can't be reviewed, diffed, or debugged when it misbehaves. Meanwhile large
protobuf codebases and graphql-codegen projects routinely commit generated
stubs precisely because the diff is the review.

**Decision.** Generated Python, TypeScript, and JSON Schema are committed.
`rspyts check` re-renders from the compiled crate and exits non-zero with a
unified diff on any drift — that is the CI gate ([codegen.md §2](codegen.md)).
Determinism rules (sorted manifests, no timestamps, content-hash headers —
codegen.md §3) exist to make those diffs minimal and meaningful.

**Consequences.** A change to the Rust contract shows up in the same PR as
readable Python/TS/Schema diffs; reviewers see exactly what downstream
callers will experience. Consumers of the generated packages need no Rust
toolchain and no codegen step. Costs: regeneration is a step contributors
can forget (CI catches it), and mass emitter changes produce large diffs
(they are still honest ones).

## ADR-4: `u64`/`i64` rejected at compile time

**Context.** JavaScript numbers are IEEE-754 doubles: integers above 2^53
silently lose precision, and JSON numbers inherit the same practical
ceiling. Options: coerce silently (corruption), serialize as strings
(surprising type changes), use `BigInt` (infects every TS call site and has
no JSON representation), or refuse.

**Decision.** Refuse, at the Rust definition site: `u64`, `i64`, `u128`,
`i128`, `usize`, `isize` simply do not implement `Bridged`
([type-system.md §1](type-system.md)), with a diagnostic suggesting
`u32`/`i32` or `String`. Explicitness over coercion — a spec decision, not
a gap. (Handles are `u64` at the ABI level but are kept below 2^53 by
construction, ABI §8.)

**Consequences.** No rspyts program can silently corrupt an integer, and
no generated field is a string that "used to be" a number. Cost: id-like
fields need `String` (ids aren't arithmetic; this is usually the better
model anyway) and genuine 64-bit quantities need explicit splitting or
re-modeling.

## ADR-5: ctypes, not the CPython C API

**Context.** A compiled extension module (C API directly, or PyO3) has the
lowest per-call overhead, but costs compiled wheels per platform (and
per-version, absent abi3), a native build toolchain in every contributor's
Python setup, and explicit GIL management.

**Decision.** The Python runtime is pure Python on `ctypes` (numpy for
buffers). Zero compiled Python code anywhere in the project: one pure wheel
runs on every platform and every CPython ≥ 3.11, including versions that
don't exist yet. `ctypes` releases the GIL around every foreign call — the
single most valuable property of a native extension — for free. Precedent:
UniFFI's Python backend made the same call, also shipping ctypes bindings
over its C ABI.

**Consequences.** Distribution is trivial (the *user's* cdylib is the only
native artifact), and long Rust calls run GIL-free so Python threads make
real progress ([Python guide](../python.md)). Cost: ctypes dispatch
is slower than a C extension per call — amortized by the design's grain
(few calls moving meaningful payloads, raw buffers for bulk data) rather
than fought with native code.

## ADR-6: Sync-only in v0.1

**Context.** Async is where the two hosts genuinely diverge. Python async
means integrating a specific event loop (the existence of
`pyo3-async-runtimes` as a separate, intricate project is the tell); JS
async means promises and microtasks (`wasm-bindgen-futures` likewise). A
C-ABI async design needs a polling/waker or completion-callback protocol
that both hosts can drive — designable, but not a detail to improvise per
runtime and reconcile later.

**Decision.** Every bridged function in v0.1 is synchronous, and there are
no callbacks from Rust into foreign code (ABI §11). Async will be one
deliberate ABI revision with identical semantics on both hosts — done
once, right.

**Consequences.** The ABI stays small and the semantics uniform. Blocking
is manageable today: the GIL is released during calls (Python threads
work), and browsers can confine calls to Web Workers. Costs: no streaming
results, no progress callbacks, no cancellation — a long call runs to
completion on its thread.

## ADR-7: Handles for stateful objects, data by value for everything else

**Context.** Sharing live objects across a language boundary drags in
cross-language identity, aliasing, and garbage-collection coordination —
the hardest problems in every FFI system. Copying everything, meanwhile, is
untenable for stateful things like accumulators, sessions, or large
internal buffers.

**Decision.** Two disjoint categories
([type-system.md §7](type-system.md)). *Data* crosses by value: serialized,
copied, no identity — a struct received in Python is just a pydantic model.
*Classes* never cross at all: state stays in Rust behind a `u64` handle
with an explicit lifecycle (context manager / `using`), per-object locking,
and idempotent drop (ABI §8). A type is data or a class, never both — the
macro enforces it.

**Consequences.** No cross-language aliasing bugs and no distributed GC by
construction, and the cost model is legible: crossing the boundary is a
copy, except the two explicit bulk paths (`&[T]` in, `Buf<T>` out). Costs:
foreign code cannot hold a mutable reference into Rust data — mutation is
a method call on a class; and handle lifetimes are the caller's
responsibility, softened by finalizer backstops in both runtimes.

## ADR-8: Types compose across crates; symbols do not (yet)

**Context.** Two different things could be "multi-crate". Sharing *types*
across bridged crates — `reports` using `catalog`'s `Annotation` with
one identity in every projection — and linking several *function-defining*
crates into one compiled module. The first is a manifest and codegen
question; the second is a symbol-naming question, because exported symbols
(`rspyts_fn__{name}`, `rspyts_cls__{Type}__{method}`) are not namespaced by
crate (ABI §3).

**Decision.** v0.1 ships cross-crate type sharing: registrations from
dependency crates link into the dependent's module, every type carries its
origin crate in the manifest, and `[python.imports]` / `[typescript.imports]`
map origins onto real import paths so foreign types are imported, never
duplicated (codegen §9). Function and class *symbols* remain one defining
crate per compiled module: identically named functions or classes from two
linked crates would collide, and `build_manifest` rejects duplicate names
loudly. Crate-prefixed symbols behind a config key stay the roadmap item.

**Consequences.** Shared vocabulary crates (pure `#[bridge]` types) compose
freely and keep one identity everywhere — `examples/multi-crate` proves it
end to end. Aggregating *behavior* from several crates into one module
still means re-exporting into a single bridged crate, or loading separate
cdylibs/WASM instances side by side (each carries its own allocator and
manifest). Cost: no single-module composition of independently owned
function-defining crates, yet.
