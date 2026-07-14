# rspyts (Python runtime)

The Python runtime for [rspyts](https://github.com/2xf-org/rspyts): define types, functions, and classes once in Rust, and `rspyts generate` emits a typed Python package — pydantic v2 models, exception hierarchies, handle classes — that calls the compiled Rust cdylib through a small C ABI.

This package is what that generated code imports: the `Contract` pydantic base, the `BridgeError` exception family, and the `Library` loader/caller. It contains no generated code, and is useful on its own only as a dependency of generated packages.

Python 3.11 through 3.14 is supported.

## Installing

```
pip install rspyts
```

Note that numpy and pydantic v2 are the only dependencies, and that the package is pure Python — there is no compiled extension module.

## Public API

- `Contract` is the unknown-field-rejecting pydantic base used by generated models.
- `Library` locates and calls the Rust `cdylib`.
- `BridgeError`, `RspytsPanicError`, and `StaleHandleError` are the runtime exception hierarchy.
- `BridgeErrorRegistry` is the call-scoped error map used by generated wrappers;
  `register_error` remains an optional process-wide fallback for handwritten code.
- `I64`/`U64` and their `*_to_wire`/`*_from_wire` helpers preserve exact
  64-bit values as Python `int` while generated code uses canonical decimal
  strings on the wire.
- `JsonValue` and the JSON helpers preserve schemaless values without
  confusing attachment-shaped user objects with protocol markers.

Applications normally import their generated package rather than calling `Library` directly.

## Documentation

Documentation can be found in the repository: the [Python guide](https://github.com/2xf-org/rspyts/blob/main/docs/python.md), and the normative specs under [docs/design](https://github.com/2xf-org/rspyts/tree/main/docs/design).

## License

This project is licensed under the MIT license.
