# TypeScript

What `rspyts generate` gives you in TypeScript, and how to run it in browsers and Node. The generated layout and the runtime API are specified in [codegen.md §5](design/codegen.md); this guide is the user's view.

The npm package `rspyts` loads and talks to the WASM build of your crate (`cargo build --target wasm32-unknown-unknown`). The generated code — `types.ts`, `constants.ts`, `errors.ts`, `client.ts`, `index.ts` — is plain strict TypeScript with no dependencies beyond that runtime. Examples below use the [basic example](../examples/basic)'s contract — `summarize`, `scale`, and the `Counter` class — plus a `Recording` class and a couple of constants that the basic crate doesn't ship, to illustrate factories and bridged consts.

## Instantiating the module

`instantiate` accepts whatever form of the `.wasm` bytes your environment produces — a `Response` (or a promise of one, for streaming compilation), a `BufferSource`, or a precompiled `WebAssembly.Module` — verifies the ABI version, and returns a `BridgeModule`.

Vite and other modern bundlers — import the asset URL and let the browser stream-compile:

```ts
import { instantiate } from "rspyts";
import wasmUrl from "./basic_example.wasm?url";

const mod = await instantiate(fetch(wasmUrl));
```

Node — read the bytes:

```ts
import { readFile } from "node:fs/promises";

const mod = await instantiate(await readFile(wasmPath));
```

Without bundler support, `new URL("./basic_example.wasm", import.meta.url)` resolves the asset relative to the current module. Note that streaming compilation requires the server to send `Content-Type: application/wasm` — check that before blaming anything else.

If you compile once and cache the `WebAssembly.Module`, pass it straight in. Each `instantiate` call produces an independent instance with its own linear memory and its own handles; instances share nothing.

## The client

`createClient(mod)` returns an object with one typed method per bridged function and one class per bridged impl block, all bound to that module instance:

```ts
import { createClient } from "./generated";

const client = createClient(mod);

const summary = client.summarize(samples, "demo");   // { itemCount, total, average, label }
const doubled = client.scale(samples, 2);            // Float64Array
const counter = new client.Counter(10, "demo");
```

Calls are synchronous — WASM execution blocks the calling thread. For heavy workloads in a browser, instantiate and call inside a Web Worker; the API is identical there.

Structs are plain interfaces with camelCase properties; `Option<T>` fields are `T | null` and optional on input. String enums are string-literal unions. Data enums are discriminated unions that narrow on their tag:

```ts
switch (parsed.type) {
  case "integer": return parsed.value;   // narrowed to the Integer variant
  case "decimal": return Math.round(parsed.value);
}
```

Statics — including factories, statics that return `Self` in Rust — live on the class bound by the client:

```ts
using rec = client.Recording.open("night-1.edf");   // factory: a fresh handle
console.log(rec.durationS());
```

A factory-only class has no `new` signature at all; `new client.Recording(...)` throws. A factory-made instance disposes exactly like a constructed one.

## Constants

Bridged `const` items need no module instance — they are plain exports with their values baked in at generation time, `as const` for maximal narrowing:

```ts
import { CHANNEL_LABELS, SAMPLE_RATE_HZ } from "./generated";

const window = SAMPLE_RATE_HZ * 30;   // SAMPLE_RATE_HZ: 256 (a literal type)
```

## Schemaless payloads — `rspyts::Json`

A Rust `rspyts::Json` field or parameter types as `unknown` — deliberately not `any`. The compiler forces you to narrow or validate before touching it, which is honest: nothing checked its shape at the boundary. Prefer real bridged types wherever the shape is known.

## Typed arrays in and out

Rust `&[T]` parameters demand the matching typed array — `Float64Array` for `&[f64]`, `Int32Array` for `&[i32]`, and so on. Plain `number[]` is **not** accepted; the conversion cost should be visible in your code, not hidden in the runtime. The runtime copies the array into an aligned allocation in linear memory for the call and frees it afterwards.

`Buf<T>` returns come back as typed arrays **copied out** of linear memory. This is deliberate: linear memory can grow and move, so views into it are never retained. What you get is safely yours, forever.

## Handles

Class instances are `u64` handles into Rust state, held as `bigint` internally. Rust memory is invisible to the JS garbage collector, so disposal is your job:

```ts
// Best: explicit resource management (TS 5.2+)
{
  using counter = new client.Counter(10, "demo");
  counter.increment(5);
  console.log(counter.currentValue());
} // counter[Symbol.dispose]() → freed, deterministically

// Equivalent, manual:
const counter = new client.Counter(10, "demo");
try {
  counter.increment(5);
} finally {
  counter.free();
}
```

`free()` and `Symbol.dispose` are idempotent — double-free is a no-op by ABI construction. Calling a method after `free()` throws `StaleHandleError`, an ordinary error, never memory corruption. A `FinalizationRegistry` frees leaked handles if the GC ever collects the wrapper, but it is a backstop, not a strategy: the GC has no idea how much Rust memory a handle pins and may never run. Always `using` or `free()`.

## Errors

Error envelopes throw generated classes; check with `instanceof`:

```ts
import { RspytsError } from "rspyts";
import { BasicError, BasicErrorEmptyInput } from "./generated";

try {
  client.summarize(samples, null);
} catch (e) {
  if (e instanceof BasicErrorEmptyInput) {
    // this specific variant; .data keys are wire-cased
  } else if (e instanceof BasicError) {
    // any other variant of this enum
  } else if (e instanceof RspytsError) {
    // bridge-level: .code, .message, .data
  } else {
    throw e;
  }
}
```

Every generated class extends `RspytsError`, which carries `.code` and `.data` and puts the human-readable message in `Error.message`.

## Types from other crates

When your bridged crate depends on another bridged crate, map the dependency's crate name to its own generated module in `rspyts.toml`:

```toml
[typescript.imports]
"example-catalog" = "@example/catalog/generated"
```

The generated `types.ts` then imports instead of re-declaring — exactly the line you would write yourself:

```ts
import type { CatalogInfo } from "@example/catalog/generated";
```

so the type is *the same type* in both packages, and the dependency's error classes keep working with `instanceof` across them. An origin with no mapping is emitted locally: self-contained, but structurally-identical-yet-distinct declarations. Details in [codegen.md §9](design/codegen.md); a working two-crate setup lives at [examples/multi-crate](../examples/multi-crate).

One caveat. On native targets a Rust panic surfaces as `RspytsPanicError`; on `wasm32-unknown-unknown` the panic strategy is *abort*, so a panic usually **traps the instance** first — you get a `WebAssembly.RuntimeError` instead. After a trap, treat the module as poisoned: discard it and `instantiate` a fresh one (keep the compiled `WebAssembly.Module` around to make that cheap). Either way, a panic is a Rust bug. The boundary contains it; it doesn't excuse it.

## Questions & Answers

**How do I bundle the** `.wasm` **file?** Keep it an asset and fetch it — `?url` in Vite. Do not inline it as base64 unless it is tiny: that defeats streaming compilation and bloats the JS bundle by ~33%. Instantiate once and share the client; a lazy singleton promise works:

```ts
let clientP: Promise<Client> | undefined;
export const getClient = () =>
  (clientP ??= instantiate(fetch(wasmUrl)).then(createClient));
```

**Can I** `await` **a bridged call?** No. Calls are synchronous in v0.1, and there are no callbacks from Rust into JS ([ADR-6](design/decisions.md#adr-6-sync-only-in-v01)). Long-running calls belong in a Web Worker.

**Why won't it take my** `number[]`**?** Because converting it costs a copy, and hidden copies are how hot paths die. Build a `Float64Array` yourself — `Float64Array.from(values)` — and the cost is visible at the call site.

**Where did** `u64` **go?** Nowhere good, which is why it isn't bridgeable: numbers above 2^53 lose precision in JS, and `BigInt` would infect every call site ([ADR-4](design/decisions.md#adr-4-u64i64-rejected-at-compile-time)). Handles use `bigint` internally, but that never surfaces in your types.

**What targets does the generated code assume?** ES2022 or later. Any evergreen browser or Node 20+ works with no feature flags. Build the crate with `--release` for shipping — debug WASM builds are large and slow — and `wasm-opt -O` from Binaryen shrinks rspyts modules like any other.
