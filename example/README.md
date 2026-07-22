# Example application

This example keeps the generated language projects beside the Rust source and
shows the automatic namespace rules.

```text
example/
├── crates/
│   └── dice/
│       ├── Cargo.toml
│       ├── rspyts.toml
│       ├── src-py/
│       ├── src-ts/
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

The `example-dice` crate owns the models and behavior. Its `rspyts.toml`
overrides the public package name while the adjacent Cargo package is linked
automatically:

```toml
[application]
name = "example"
```

The API Cargo package is `example-dice`. rspyts uses the public application
name `example`, removes that shared prefix, and then adds the Rust declaration
modules.

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
a second application package.

Build both host packages:

```sh
cargo run -p rspyts-cli -- build
```

The build updates the generated files inside
`example/crates/dice/src-py` and `example/crates/dice/src-ts`. Their generated
`.gitignore` files exclude only RSPYTS-owned outputs; authored manifests,
entrypoints, and convenience modules remain normal Git files. Run the build
before using either client.

Then run the authored clients:

```sh
uv run --project example/clients/python pytest -q example/clients/python/tests
npm --prefix example/crates/dice/src-ts ci
npm --prefix example/crates/dice/src-ts run build
npm --prefix example/clients/typescript ci
npm --prefix example/clients/typescript run check
npm --prefix example/clients/typescript run build
npm --prefix example/clients/typescript run test:integration
```

These client commands use `uv`, Python, Node.js, and npm. They are example
development tools. `rspyts build` does not require them.

Both clients roll three seeded dice. Both clients return `[5, 4, 2]` with a
total of `11`. They also check duplicate model, function, and resource names;
a cross-namespace model reference; and a cross-namespace error.
