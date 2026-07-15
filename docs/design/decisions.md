# Design decisions

These decisions describe the boundary rspyts intends to keep. They are useful
when a feature request could make the bridge broader but less predictable.

## ADR-1: One C ABI, two runtimes

Python and TypeScript consume the same compiled contract. Python uses a native
cdylib through `ctypes`; TypeScript uses WebAssembly exports.

This keeps semantics in one place. A value, error, or ownership rule should
not mean one thing in Python and another in TypeScript.

## ADR-2: JSON as the structured wire format

Structured values use JSON because it is inspectable, stable across hosts, and
already supported by Serde. Binary attachments keep arrays and bytes out of
JSON when copying and encoding them would dominate a call.

The tradeoff is deliberate: rspyts optimizes bulk data, not every scalar call.

## ADR-3: Generated code is committed

Generated clients are source artifacts. They belong in review, type checking,
package builds, and downstream diffs.

This decision covers generated Python/TypeScript source and any configured
schema output, not staged native libraries or WebAssembly modules. Those are
platform build artifacts and should be packaged as needed rather than committed.

`rspyts check` makes drift mechanical. Consumers never need Rust or the
generator merely to install a Python or npm package.

## ADR-4: Native exact 64-bit integers

JavaScript numbers cannot represent every signed or unsigned 64-bit integer.
Domain contracts therefore use ordinary Rust `i64` and `u64`, which project to
Python `int` and TypeScript `bigint`. Schema-directed boundary codecs carry
them as canonical decimal strings without changing ordinary Rust Serde behavior.

## ADR-5: ctypes, not the CPython C API

The Python runtime loads the same stable symbols used by WebAssembly and other
native callers. It does not compile a CPython extension or depend on Python's
internal ABI.

This makes the runtime small and portable. It also means rspyts does not try to
make Rust objects subclassable Python objects.

## ADR-6: Calls remain synchronous

Every shim begins and ends in one host call. Async runtimes would need a
separate lifetime, cancellation, wakeup, and error protocol.

That work is not hidden behind a blocking wrapper. Async support should arrive
only with an explicit ABI design.

## ADR-7: Handles for stateful objects, values for data

Structs, enums, lists, maps, tuples, buffers, and errors cross by value.
Stateful classes stay in Rust behind opaque handles.

This prevents foreign references into Rust memory. Handles are synchronized,
nonzero, never reused, and explicitly disposable.

## ADR-8: Types compose across crates; symbols do not

A manifest records each type's origin. Generated packages may import a type
from another crate's generated package. Python then reuses the same runtime
model class. TypeScript reuses the same declaration provenance and source of
truth; its structural type system does not provide nominal interface identity.

Calls remain bound to one loaded module. Cross-module symbol routing would need
a linker-level contract and is not inferred by code generation.

## ADR-9: One reflected Serde vocabulary, two ownership modes

Default `#[bridge]` owns a predictable Serde shape: camelCase names and unknown
field rejection. `#[bridge(serde)]` adopts an existing derived contract.

Adoption reflects only shape-preserving attributes that the manifest can model
exactly. `flatten`, `untagged`, aliases, defaults, skipped fields, and custom
codecs are rejected. Schemaless data uses `serde_json::Value` explicitly.

## ADR-10: ABI 3 uses one envelope in both directions

Requests and responses share a strict 12-byte header, JSON payload, and binary
tail. Direction-specific validation interprets byte zero as a request marker or
response status.

Symmetry removes the old split between raw request JSON and framed responses.
It also allows nested owned buffers on input without adding per-function ABI
parameters.

## ADR-11: Exact 64-bit values are native Rust types with an exact wire form

Domain code uses ordinary `i64` and `u64`. The schema-directed Rust boundary
converts them to canonical decimal strings only while crossing the ABI, so
ordinary Serde remains ordinary and neither foreign host loses precision.

This rule applies recursively to fields, collections, tuples, constants, and
error data. Numeric buffer attachments remain raw fixed-width integers because
their declared dtype already carries the exact contract.

## What would justify a new decision

A new shape belongs in rspyts only when all of these are clear:

- its Rust, wire, Python, TypeScript, and schema meanings agree;
- ownership survives panics, traps, threads, and garbage collection;
- invalid values fail before user Rust runs;
- code generation remains deterministic;
- the behavior can be tested end to end on native and WebAssembly targets.

If those answers require a project-specific exception, the feature does not
belong in the general package.
