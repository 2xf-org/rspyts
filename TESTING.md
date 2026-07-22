# Testing rspyts

rspyts tests behavior through public boundaries. The repository does not keep
in-module unit-test suites or shallow smoke checks.

## Test boundaries

* `crates/rspyts/tests` compiles the public macros as a downstream crate,
  discovers the resulting contract, and calls the registered Python boundary.
* `crates/rspyts-cli/tests` exercises explicit CLI arguments and output, then
  runs the compiled `rspyts` binary for process-level behavior.
* `example/clients/python` tests the generated Python package and native
  extension. Mypy and Pyright check the same public package statically.
* `example/clients/typescript` type-checks and runs the generated TypeScript and
  WebAssembly package.

The CLI library separates argument syntax (`cli.rs`), command orchestration
(`commands.rs`), and process I/O (`lib.rs`). Integrations can call
`rspyts_cli::run_with` with explicit arguments and output writers. Tests that
need exit status or process stderr should invoke the compiled binary instead.

`watch` intentionally remains a long-running process command. Its polling and
rebuild loop is isolated in the command orchestration module; process-level
coverage should start the binary, make a source change, observe the rebuild,
and terminate the child with a bounded timeout.

## Running integrations

Run Rust and CLI integrations:

```sh
cargo test --locked -p rspyts --test contract --test python
cargo test --locked -p rspyts-cli --test cli
```

Build the generated packages before running the language clients:

```sh
cargo run --locked -p rspyts-cli -- build
uv run --frozen --project example/clients/python pytest -q example/clients/python/tests
uv run --frozen --project example/clients/python mypy example/clients/python/typecheck.py
uv run --frozen --project example/clients/python pyright example/clients/python/typecheck.py
npm --prefix example/clients/typescript run check
npm --prefix example/clients/typescript run build
npm --prefix example/clients/typescript run test:integration
```

New tests should cross a documented public boundary and assert a meaningful
success or failure contract. Prefer extending an existing integration fixture
over exposing internal rendering or validation helpers solely for tests.
