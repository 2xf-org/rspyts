# Example application

This example uses one aggregate binding and one linked Rust domain crate. It
shows the automatic namespace rules.

```text
example/
├── crates/
│   ├── bindings/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── dice/
│       ├── Cargo.toml
│       └── src/
│           ├── fair/roll.rs
│           ├── loaded/roll.rs
│           ├── lib.rs
│           └── summary.rs
└── clients/
    ├── python/
    │   ├── example_client/__init__.py
    │   ├── tests/test_client.py
    │   ├── pyproject.toml
    │   └── uv.lock
    └── typescript/
        ├── src/index.ts
        ├── package-lock.json
        ├── package.json
        └── tsconfig.json
```

The `example-dice` crate owns the models and behavior. The `example` crate
contains only this application declaration:

```rust
rspyts::application!(example_dice);
```

The binding Cargo package is `example`. The API Cargo package is
`example-dice`. rspyts removes the shared `example` prefix. It then adds the
Rust declaration modules.

Use these Python imports:

```python
from example.dice.fair.roll import DiceCup, RollResult
from example.dice.loaded.roll import DiceCup as LoadedDiceCup
from example.dice.loaded.roll import RollResult as LoadedRollResult
from example.dice.loaded.roll import roll_dice as loaded_roll_dice
from example.dice.summary import summarize_roll
```

Use these TypeScript imports:

```typescript
import { DiceCup, type RollResult } from "example/dice/fair/roll";
import {
  DiceCup as LoadedDiceCup,
  rollDice as loadedRollDice,
  type RollResult as LoadedRollResult,
} from "example/dice/loaded/roll";
import { summarizeRoll } from "example/dice/summary";
```

The two modules reuse the `DiceCup`, `RollResult`, and `roll_dice` leaf names.
They are valid because they belong to different namespaces. `RollSummary`
contains the fair result. This reference crosses a namespace boundary without
a second binding.

Build both host packages:

```sh
cargo run -p rspyts-cli -- build
```

The build creates the generated Python and TypeScript packages in
`example/crates/bindings/dist`. Git stores this generated snapshot, including
the native Python extension and the WebAssembly binary. Run the build again to
replace the snapshot with binaries for your operating system.

Then run the authored clients:

```sh
uv run --project example/clients/python pytest -q example/clients/python/tests
npm --prefix example/clients/typescript ci
npm --prefix example/clients/typescript run check
npm --prefix example/clients/typescript run build
npm --prefix example/clients/typescript run start
```

These client commands use `uv`, Python, Node.js, and npm. They are example
development tools. `rspyts build` does not require them.

Both clients roll three seeded dice. Both clients return `[5, 4, 2]` with a
total of `11`. They also check duplicate model, function, and resource names;
a cross-namespace model reference; and a cross-namespace error.
