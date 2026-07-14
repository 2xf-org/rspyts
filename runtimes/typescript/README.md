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
npm install rspyts
```

## Public API

- `instantiate` validates and wraps an rspyts WebAssembly module.
- `BridgeModule` describes the validated module and its memory.
- `callFn` and `callDrop` are used by generated clients.
- `RspytsError`, `RspytsPanicError`, and `StaleHandleError` form the bridge error hierarchy.
- `InstancePoisonedError` stops calls after a WebAssembly runtime trap; create
  a fresh module instance before retrying.
- `i64ToWire`, `u64ToWire`, `i64FromWire`, and `u64FromWire` are the
  range-checked exact-`bigint` conversion surface used by generated clients.

The package exports only the support surface required by generated code; application code should normally use its generated `createClient`.

## Documentation

Documentation can be found in the repository: the [TypeScript guide](https://github.com/2xf-org/rspyts/blob/main/docs/typescript.md), and the normative specs under [docs/design](https://github.com/2xf-org/rspyts/tree/main/docs/design).

## Development

```
npm ci
npm run typecheck
npm test
npm run build
npm run check:surface
```

## License

This project is licensed under the MIT license.
