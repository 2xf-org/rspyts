# Static contracts and one direct owner

This guide covers the other two supported shapes:

1. a static leaf that emits Python source and static TypeScript; and
2. a static consumer that imports one direct WASM leaf through its canonical
   `./wire` export.

The runnable fixtures are [`examples/static`](../examples/static) and
[`examples/cross-package`](../examples/cross-package).

## Static leaf

Use this shape for Rust-owned identifiers, schemas, constants, and tables that
JavaScript reads without executing Rust.

```toml
[crate]
path = "rust"

[python]
package = "example.protocol"
source = "python/src"

[typescript]
package = "@example/protocol"
mode = "static"
```

The crate still declares exactly one module. Static exports are ordinary Rust
types and values:

```rust
use rspyts::Type;
use serde::Serialize;

#[derive(Clone, Copy, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Stable,
    Beta,
}

#[derive(Clone, Copy, Serialize, Type)]
#[serde(transparent)]
pub struct FormatMagic(#[rspyts(bytes)] pub [u8; 4]);

#[rspyts::export(static)]
pub const FORMAT_VERSION: u32 = 4;

#[rspyts::export(static)]
pub const FORMAT_MAGIC: FormatMagic = FormatMagic(*b"RSPY");

#[rspyts::export(static)]
pub const RELEASE_CHANNELS: &[ReleaseChannel] = &[
    ReleaseChannel::Stable,
    ReleaseChannel::Beta,
];

rspyts::module!(native);
```

Build and accept the semantic contract:

```sh
rspyts build
rspyts lock
rspyts check --locked
```

The generated static package is ordinary ESM plus declarations:

```ts
export enum ReleaseChannel {
  Stable = "stable",
  Beta = "beta",
}

export const FORMAT_VERSION: 4;
export const FORMAT_MAGIC: readonly [82, 83, 80, 89];
export const RELEASE_CHANNELS: readonly [
  ReleaseChannel.Stable,
  ReleaseChannel.Beta,
];
```

Objects, arrays, and tuples are deeply frozen. Fixed byte arrays retain their
exact length. Static `i64` and `u64` values must be exact JavaScript safe
integers. Functions and resources cannot target static TypeScript.

Python remains generated source for a Maturin wheel. The optional authored
`python.source` directory is copied into `.rspyts/python`; output always stays
at the fixed `.rspyts/` path.

## Static consumer with one WASM owner

A consumer may import one direct leaf contract. The owner must have no contract
dependency of its own and must expose a WASM TypeScript package so the static
consumer can import its `./wire` surface.

Owner configuration:

```toml
[crate]
path = "rust"

[python]
package = "example.owner"
source = "python/src"

[typescript]
package = "@example/owner"
mode = "wasm"
```

Consumer configuration:

```toml
[crate]
path = "rust"

[python]
package = "example.consumer"
source = "python/src"

[typescript]
package = "@example/consumer"
mode = "static"

[dependencies.owner]
crate = "example-owner"
lock = "../owner/rspyts.lock"
python = "example.owner"
typescript = "@example/owner"
```

The alias `owner` is local configuration. Identity comes from the exact Cargo
package name, exact crate version, semantic fingerprint, and host package names
recorded in the owner's committed lock.

The consumer's Rust types may refer directly to owner types. rspyts emits host
imports rather than duplicate definitions:

```ts
import type { Item, Quantity } from "@example/owner/wire";
import { ItemKind } from "@example/owner/wire";

export interface Selection {
  readonly item: Item;
  readonly requested: Quantity;
  readonly expectedKind: ItemKind;
}
```

Generated Python imports the owner's generated classes and verifies the same
fingerprint before exposing the consumer package. Generated TypeScript declares
the exact owner version as a peer and performs its guard during module loading.
Neither host generates a second owner model.

## Build order

Accept the leaf first, then the consumer:

```sh
rspyts build --config owner/rspyts.toml
rspyts lock --config owner/rspyts.toml
rspyts check --locked --config owner/rspyts.toml

rspyts build --config consumer/rspyts.toml
rspyts lock --config consumer/rspyts.toml
rspyts check --locked --config consumer/rspyts.toml
```

Commit both `rspyts.lock` files and ignore both `.rspyts/` directories. When the
owner changes, review and accept its lock before rebuilding the consumer.

## Dependency limits

- A contract has zero or one direct dependency.
- A dependency lock must be a leaf: it cannot contain another contract import.
- One Cargo package name resolves to one exact version in the graph.
- Aliases cannot create a second identity or distinguish package versions.
- Stale fingerprints, undeclared imported types, duplicate host package names,
  and dependency cycles are errors.
- The supported direct-consumer shape imports one WASM leaf's `./wire`.

Generated output never becomes an input. Depend on the committed semantic lock,
not another package's `.rspyts/` directory.
