# Sensor readings example

This is a complete rspyts project, not a standalone test fixture. Its Rust library summarizes, normalizes, encodes, and decodes sensor readings. The generated Python extension and TypeScript WebAssembly package expose the same API, while each language adds one handwritten `describe_readings` helper.

```text
example/
├── Cargo.toml
├── rspyts.toml
├── src/
│   └── lib.rs
├── src-py/
│   ├── pyproject.toml
│   ├── example/
│   │   ├── __init__.py
│   │   └── convenience/
│   │       └── __init__.py
│   └── tests/
│       ├── test_boundaries.py
│       └── test_readings.py
└── src-ts/
    ├── package.json
    ├── tsconfig.json
    ├── example/
    │   ├── index.ts
    │   └── convenience/
    │       └── index.ts
    └── tests/
        ├── boundaries.test.mjs
        └── readings.test.mjs
```

## Build

From the repository root:

```console
cargo run --locked -p rspyts-cli -- build
cargo run --locked -p rspyts-cli -- check
```

rspyts generates the Python models, API, runtime, and native extension under `src-py/example`. It generates the equivalent TypeScript sources and WebAssembly module under `src-ts`. The manifests, package entrypoints, convenience modules, and tests remain user-owned.

## Python

```python
import numpy as np

from example import describe_readings, normalize_readings

readings = np.array([12.0, 18.0, 15.0, 21.0], dtype=np.float64)
print(describe_readings(readings))
print(normalize_readings(readings))
```

Install and test the package with standard Python tooling:

```console
python -m pip install -e 'example/src-py[dev]'
python -m pytest -q example/src-py/tests
```

## TypeScript

```typescript
import { describeReadings, normalizeReadings } from "example";

const readings = new Float64Array([12, 18, 15, 21]);
console.log(describeReadings(readings));
console.log(normalizeReadings(readings));
```

Build and test the WebAssembly package with npm:

```console
npm --prefix example/src-ts install
npm --prefix example/src-ts run check
npm --prefix example/src-ts run build
npm --prefix example/src-ts test
```

## What the tests cover

Rust unit tests verify the statistics, normalization, binary format, and invalid-input behavior without crossing a language boundary. Python and TypeScript integration tests exercise the installed native and WebAssembly packages, including generated models, typed errors, handwritten helpers, and exact byte/buffer results. Dedicated large-array regressions verify that the direct binary boundaries remain fast without adding artificial exports to the example API.
