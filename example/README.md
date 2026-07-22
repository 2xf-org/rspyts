# Example

This directory is one complete rspyts application. Rust, Python, and TypeScript source live together without a separate consumer project.

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
│   ├── tests/
│   │   └── test_example.py
│   └── typecheck.py
└── src-ts/
    ├── package.json
    ├── tsconfig.json
    └── example/
        ├── index.ts
        └── convenience/
            └── index.ts
```

## Build

Run the generator from the repository root:

```console
cargo run --locked -p rspyts-cli -- build
cargo run --locked -p rspyts-cli -- check
```

The build adds generated models, APIs, and runtimes beside the authored language code. It places the Python extension in `src-py/example/native` and the Wasm module directly in `src-ts/build/example/native`; neither language package needs an asset-copy script.

## Python

```console
python -m pip install -e example/src-py
python -c 'from example import excited_greeting, greet; print(greet("Ada").message); print(excited_greeting("Ada"))'
```

`greet` comes from Rust. `excited_greeting` is handwritten in `src-py/example/convenience/__init__.py` and calls the generated function.

## TypeScript

```console
npm --prefix example/src-ts install
npm --prefix example/src-ts run check
npm --prefix example/src-ts run build
node --input-type=module --eval 'const { excitedGreeting, greet } = await import("./example/src-ts/build/example/index.js"); console.log(greet("Ada").message); console.log(excitedGreeting("Ada"));'
```

The same Rust `greet` function runs through WebAssembly. The handwritten `src-ts/example/convenience/index.ts` module calls it and is re-exported by the package entrypoint.

## Change the example

Edit `src/lib.rs` to change the generated API. Add ordinary Python or TypeScript modules under `src-py/example/convenience` or `src-ts/example/convenience`; rspyts preserves every path not listed in `rspyts.toml`.
