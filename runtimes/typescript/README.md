# rspyts (TypeScript runtime)

The TypeScript runtime for [rspyts](https://github.com/2xf-org/rspyts): define types, functions, and classes once in Rust, and call them from TypeScript through a WebAssembly module with fully generated typings.

This package is the small support library that code emitted by `rspyts generate` builds on — module instantiation, argument marshalling, envelope decoding, and the bridge error hierarchy. You normally use it through a generated `createClient`, not directly:

Node.js 22.12 or newer is supported, with CI coverage on Node 22 and 24. Browser use relies only on standard WebAssembly APIs.

```ts
import { instantiate } from "rspyts";
import { createClient } from "./generated/index.js";

const client = createClient(await instantiate(fetch("/basic_example.wasm")));

const summary = client.summarize(new Float64Array([2, 4, 6]), "demo");
```

## Installing

```
npm install rspyts@0.3.2
```

## Public API

- `instantiate(source, { expectedContractFingerprint? })` validates the ABI-3
  module, required exports, memory, and its deterministic contract
  fingerprint. Supplying the generated 64-character lowercase SHA-256 also
  rejects a stale or wrong binary during loading.
- `BridgeModule` exposes the validated exports, memory, and loaded
  `contractFingerprint`.
- `ContractFingerprintMismatchError` reports the expected and actual
  fingerprints when generated bindings and a module do not match.
- `RspytsError`, `RspytsPanicError`, and `StaleHandleError` form the bridge error hierarchy.
- `InstancePoisonedError` stops calls after a WebAssembly runtime trap; create
  a fresh module instance before retrying.

Application code should normally use this root surface plus its generated
`createClient`. Generator plumbing is deliberately isolated at
`rspyts/internal/abi3`.

## Exact emitter API

ABI-3 generated code imports only the following versioned, semver-exempt
subpath:

```ts
import {
  type BridgeErrorRegistry,
  type BridgeModule,
  type SliceArg,
  type WireResponse,
  type WireVariantShape,
  RspytsError,
  boolFromWire,
  boundedIntFromWire,
  bufferFromWire,
  bytesFromWire,
  callDrop,
  callFn,
  enumFromWire,
  f32FromWire,
  floatFromWire,
  i64FromWire,
  i64ToWire,
  jsonFromWire,
  jsonToWire,
  listFromWire,
  mapFromWire,
  nullFromWire,
  objectFromWire,
  stringEnumFromWire,
  stringFromWire,
  tupleFromWire,
  u64FromWire,
  u64ToWire,
  verifyModuleContract,
  wireBuffer,
  wireResponse,
} from "rspyts/internal/abi3";
```

The call contract has one path:

```ts
interface WireResponse<T = unknown> {
  readonly value: T;
  readonly tail: Uint8Array;
}

function callFn(
  mod: BridgeModule,
  symbol: string,
  args: unknown,
  slices?: SliceArg[],
  handle?: bigint,
  errorTypes?: BridgeErrorRegistry,
): WireResponse;

function callDrop(mod: BridgeModule, symbol: string, handle: bigint): void;
function verifyModuleContract(mod: BridgeModule, expected: string): void;
```

Every successful-return decoder consumes `WireResponse`, never a bare value.
Scalar decoders return their host scalar. `listFromWire` and `tupleFromWire`
return `WireResponse[]`; `mapFromWire`, `objectFromWire`, and `enumFromWire`
return records whose values are child `WireResponse` objects carrying the same
explicit tail. `bufferFromWire` and `bytesFromWire` validate exact wrapper
shape, dtype, alignment, and bounds before copying attachment bytes. The
remaining decoder signatures are recorded in `etc/public-surface.d.ts` and
checked in CI.

`wireBuffer` brands numeric request attachments with a collision-proof host
symbol. `jsonToWire` and `jsonFromWire` validate and copy ordinary JSON without
adding a wire wrapper: reserved-looking keys stay application data at a
schema-declared `Json` position. `wireResponse(value)` creates empty-tail
context for generated error-data conversion.

Generated `createClient` calls
`verifyModuleContract(mod, "<manifest sha256>")` before exposing operations.
The module export `rspyts_contract_fingerprint()` is an owned success envelope
whose JSON value is the exact lowercase 64-character SHA-256 and whose tail is
empty; the runtime copies and frees it with normal ABI ownership rules.

## Documentation

Documentation can be found in the repository: the [TypeScript guide](https://github.com/2xf-org/rspyts/blob/main/docs/typescript.md), and the normative specs under [docs/design](https://github.com/2xf-org/rspyts/tree/main/docs/design).

## Development

```
npm ci
npm run typecheck
npm run lint
npm test
npm run build
npm run check:surface
```

## License

This project is licensed under the MIT license.
