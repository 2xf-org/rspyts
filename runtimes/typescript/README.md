# rspyts (TypeScript runtime)

The TypeScript runtime for [rspyts](https://github.com/2xf-org/rspyts): define types, functions, and classes once in Rust, and call them from TypeScript through a WebAssembly module with fully generated typings.

This package is the small support library that code emitted by `rspyts generate` builds on — module instantiation, argument marshalling, envelope decoding, and the bridge error hierarchy. You normally use it through a generated `createClient`, not directly:

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

## Documentation

Documentation can be found in the repository: the [TypeScript guide](https://github.com/2xf-org/rspyts/blob/main/docs/typescript.md), and the normative specs under [docs/design](https://github.com/2xf-org/rspyts/tree/main/docs/design).

## Development

```
npm install
npm run typecheck
npm test
npm run build
```

## License

This project is licensed under the MIT license.
