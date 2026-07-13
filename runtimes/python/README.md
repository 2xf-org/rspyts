# rspyts (Python runtime)

The Python runtime for [rspyts](https://github.com/2xf-org/rspyts): define types, functions, and classes once in Rust, and `rspyts generate` emits a typed Python package — pydantic v2 models, exception hierarchies, handle classes — that calls the compiled Rust cdylib through a small C ABI.

This package is what that generated code imports: the `Contract` pydantic base, the `BridgeError` exception family, and the `Library` loader/caller. It contains no generated code, and is useful on its own only as a dependency of generated packages.

## Installing

```
pip install rspyts
```

Note that numpy and pydantic v2 are the only dependencies, and that the package is pure Python — there is no compiled extension module.

## Documentation

Documentation can be found in the repository: the [Python guide](https://github.com/2xf-org/rspyts/blob/main/docs/python.md), and the normative specs under [docs/design](https://github.com/2xf-org/rspyts/tree/main/docs/design).

## License

This project is licensed under the MIT license.
