# Static TypeScript and package dependencies

Two common cases do not need handwritten bridge models:

1. Rust owns constants, enums, and lookup tables that TypeScript should read.
2. One Rust package uses a type owned by another Rust package, and the generated
   Python and TypeScript packages should preserve that ownership.

The runnable projects are
[`examples/static`](../examples/static) and
[`examples/cross-package`](../examples/cross-package).

Generated packages go in `.rspyts/` and stay out of Git. `rspyts.lock` is the
small, reviewable semantic contract and belongs in Git.

```gitignore
.rspyts/
.rspyts.tmp-*
.rspyts.old-*
.rspyts.lock.tmp-*
.rspyts.lock.old-*
```

## Static TypeScript without WASM

Static mode evaluates exported Rust constants while building and emits ordinary
JavaScript plus TypeScript declarations. It has no `.wasm` file, initialization
step, wasm-bindgen dependency, or rspyts runtime dependency.

This is a good fit for protocol versions, event names, headers, feature tables,
and other vocabulary that Rust already owns.

### 1. Configure a static package

The complete configuration is intentionally small:

```toml
# examples/static/rspyts.toml
[crate]
path = "rust"

[typescript]
package = "@rspyts/static-acceptance"
mode = "static"
```

Static mode needs `serde` and `rspyts` in the Rust crate, but it does not need a
`wasm` Cargo feature, a direct wasm-bindgen dependency, the
`wasm32-unknown-unknown` target, or the wasm-bindgen CLI.

### 2. Author the contract in Rust

Derive `rspyts::Type` on the real Rust types and mark the values that should be
available to TypeScript with `#[rspyts::export(static)]`.

```rust
use rspyts::Type;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PlatformEvent {
    PortalHeartbeat,
    StudyOpened,
    SessionClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MeterDefinition {
    pub event: PlatformEvent,
    pub meter: &'static str,
    pub unit: &'static str,
    pub billable: bool,
}

#[rspyts::export(static)]
pub const PROTOCOL_VERSION: u32 = 4;

#[rspyts::export(static)]
pub const PROTOCOL_EPOCH: u64 = 4_294_967_296;

#[rspyts::export(static)]
pub const SUPPORTED_EVENTS: &[PlatformEvent] = &[
    PlatformEvent::PortalHeartbeat,
    PlatformEvent::StudyOpened,
    PlatformEvent::SessionClosed,
];

#[rspyts::export(static)]
pub const METER_TABLE: &[(&str, &str)] = &[
    ("portal_heartbeat", "portal.active_seconds"),
    ("study_opened", "study.opens"),
    ("session_closed", "session.seconds"),
];

rspyts::module!(native);
```

Serde naming remains the wire format: `PortalHeartbeat` becomes
`"portal_heartbeat"`, while a field such as `protocol_version` becomes
`protocolVersion`. All `i64` and `u64` values are emitted as `bigint`, regardless
of magnitude; the `u64` epoch above becomes `4294967296n`.

rspyts does not translate Rust function bodies. If TypeScript must execute Rust,
use `mode = "wasm"` instead. If it only needs the values and types, keep it
static.

### 3. Build the package

From the repository root:

```sh
rspyts build \
  --config examples/static/rspyts.toml
```

The build writes `contract.json`, `index.d.ts`, `index.js`, and `package.json`
below `examples/static/.rspyts/`.

The declaration output retains exact literal and readonly types. This is an
abridged excerpt of the generated `index.d.ts`:

```ts
export enum PlatformEvent {
  PortalHeartbeat = "portal_heartbeat",
  StudyOpened = "study_opened",
  SessionClosed = "session_closed",
}

export interface MeterDefinition {
  readonly event: PlatformEvent;
  readonly meter: string;
  readonly unit: string;
  readonly billable: boolean;
}

export const PROTOCOL_VERSION: 4;
export const PROTOCOL_EPOCH: 4294967296n;
export const SUPPORTED_EVENTS: readonly [
  PlatformEvent.PortalHeartbeat,
  PlatformEvent.StudyOpened,
  PlatformEvent.SessionClosed,
];
export const METER_TABLE: readonly [
  readonly ["portal_heartbeat", "portal.active_seconds"],
  readonly ["study_opened", "study.opens"],
  readonly ["session_closed", "session.seconds"],
];
```

The generated `index.js` is ordinary ESM. This excerpt is abridged; the
generated `deepFreeze` helper is omitted:

```js
export const PlatformEvent = Object.freeze({
  PortalHeartbeat: "portal_heartbeat",
  StudyOpened: "study_opened",
  SessionClosed: "session_closed",
});

export const PROTOCOL_VERSION = deepFreeze(4);
export const PROTOCOL_EPOCH = deepFreeze(4294967296n);
export const SUPPORTED_EVENTS = deepFreeze([
  "portal_heartbeat",
  "study_opened",
  "session_closed",
]);
export const METER_TABLE = deepFreeze([
  ["portal_heartbeat", "portal.active_seconds"],
  ["study_opened", "study.opens"],
  ["session_closed", "session.seconds"],
]);
```

Objects, arrays, and nested tuples are deeply frozen. All typed-array and
`DataView` views deliberately remain unfrozen. Consumers get the exact Rust
values rather than a second handwritten table:

```ts
import {
  PlatformEvent,
  PROTOCOL_VERSION,
  SUPPORTED_EVENTS,
} from "@rspyts/static-acceptance";

const current = SUPPORTED_EVENTS[0] === PlatformEvent.PortalHeartbeat;
console.log(PROTOCOL_VERSION, current);
```

The consuming repository decides how `.rspyts/typescript` enters its package
artifact. Do not commit it as source.

### 4. Accept the semantic contract

Once the generated package and its consumer tests look right, write the lock:

```sh
rspyts lock \
  --config examples/static/rspyts.toml

git add examples/static/rspyts.toml examples/static/rspyts.lock
```

The lock contains normalized types, values, host identities, and a semantic
fingerprint. Review it instead of generated JavaScript and declarations.

CI should build the current contract and require an exact lock match:

```sh
rspyts check --locked \
  --config examples/static/rspyts.toml
```

When a Rust constant changes, the locked check fails. Rebuild, inspect the
consumer behavior, run `rspyts lock`, and commit the semantic lock update as the
public contract change.

## Cross-package Rust type identity

Suppose Catalog defines `SignalDefinition`, and Reports returns an
`ReportsResult` containing it. Do not copy that model into Reports and
maintain conversions in three languages.

rspyts instead records the Cargo package that owns every registered Rust type.
Reports can use Catalog's real Rust type directly. Its generated packages
then import Catalog's generated host type.

The generated Reports packages import `SignalDefinition` from
`example.catalog` and `@example/catalog`. They do not depend on
Catalog's `.rspyts/` staging directory. The lock verifies Rust contract
compatibility at compile time; it does not pin released Python or npm versions.

### 1. Define the owner package

Catalog owns the Rust type and its host package names:

```toml
# examples/cross-package/catalog/rspyts.toml
[crate]
path = "rust"
features = ["bindings"]

[python]
package = "example.catalog"

[typescript]
package = "@example/catalog"
mode = "static"
```

```rust
use rspyts::Type;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignalDefinition {
    pub id: String,
    pub sample_rate_hz: u32,
}

#[rspyts::export]
pub fn define_signal(id: String, sample_rate_hz: u32) -> SignalDefinition {
    SignalDefinition { id, sample_rate_hz }
}

#[cfg(feature = "bindings")]
rspyts::module!(native);
```

Lock Catalog before a dependent package tries to resolve it:

```sh
rspyts lock \
  --config examples/cross-package/catalog/rspyts.toml

rspyts build \
  --config examples/cross-package/catalog/rspyts.toml
```

The generated Catalog package owns the host models. Abridged:

```python
# example/catalog/models.py
class SignalDefinition(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
        extra="forbid",
        frozen=True,
    )
    id: str
    sample_rate_hz: int = Field(alias="sampleRateHz")
```

```ts
// @example/catalog/index.d.ts
export interface SignalDefinition {
  readonly id: string;
  readonly sampleRateHz: number;
}
```

### 2. Map the dependency

Reports has a normal Cargo dependency on Catalog and refers to its real Rust
type:

```rust
use cross_package_catalog::SignalDefinition;
use rspyts::Type;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportsResult {
    pub signal: SignalDefinition,
    pub accepted: bool,
}

#[rspyts::export]
pub fn evaluate(signal: SignalDefinition, score: i32) -> ReportsResult {
    ReportsResult {
        signal,
        accepted: score >= 0,
    }
}

rspyts::module!(native);
```

Its rspyts configuration maps the foreign Cargo owner to the committed Catalog
lock and the package names that consumers install:

```toml
# examples/cross-package/reports/rspyts.toml
[crate]
path = "rust"

[python]
package = "example.reports"

[typescript]
package = "@example/reports"
mode = "static"

[dependencies.catalog]
crate = "cross-package-catalog"
lock = "../catalog/rspyts.lock"
python = "example.catalog"
typescript = "@example/catalog"
```

`catalog` is a local alias. `crate` must match the Cargo package owner captured
when `SignalDefinition` was registered; `lock` is relative to Reports's
configuration.

At compile time, rspyts reads Catalog's lock, verifies the referenced type and
fingerprint, and records the dependency's owner, host names, fingerprint, and
referenced shape in Reports's lock. That shape is verification data, not a
second generated model.

### 3. Build the consumer

With Catalog's current lock available:

```sh
rspyts lock \
  --config examples/cross-package/reports/rspyts.toml

rspyts build \
  --config examples/cross-package/reports/rspyts.toml
```

Reports generates only the model it owns. This abridged Python model module
imports Catalog's class:

```python
# example/reports/models.py
from example.catalog import SignalDefinition

class ReportsResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
        extra="forbid",
        frozen=True,
    )
    signal: SignalDefinition
    accepted: bool
```

There is no second `class SignalDefinition`. At runtime both package surfaces
refer to the same Python class object:

```python
from example.reports import SignalDefinition as ReportsSignal, evaluate
from example.catalog import SignalDefinition as CatalogSignal

assert ReportsSignal is CatalogSignal

signal = CatalogSignal(id="sig:c3", sample_rate_hz=256)
result = evaluate(signal, 1)

assert isinstance(result.signal, CatalogSignal)
assert result.accepted is True
```

Reports's TypeScript declarations use the installed Catalog package too:

```ts
import type { SignalDefinition } from "@example/catalog";
export type { SignalDefinition } from "@example/catalog";

export interface ReportsResult {
  readonly signal: SignalDefinition;
  readonly accepted: boolean;
}
```

There is no copied `interface SignalDefinition` in Reports. Because this
fixture configures TypeScript as `static`, Reports's Rust `evaluate`
function is also absent from its TypeScript package. The executable function is
available through its generated Python native boundary; TypeScript receives
only the static vocabulary.

rspyts emits `"@example/catalog": "*"` in Reports's npm
`peerDependencies`. A release packager may replace that with the compatible
version range it publishes. Generated Python source contains the absolute import
but no Python distribution dependency metadata; the wheel or application
packager must declare the compatible Catalog distribution.

### 4. Check the package graph

Commit both `rspyts.lock` files, never either generated directory.

Then check owner before consumer in CI:

```sh
rspyts check --locked \
  --config examples/cross-package/catalog/rspyts.toml

rspyts check --locked \
  --config examples/cross-package/reports/rspyts.toml
```

The fixture checks emitted imports, Python identity, and stale-lock rejection:

```sh
python examples/cross-package/assert_identity.py

uv run --with 'pydantic>=2.11' --with 'numpy>=2' \
  python examples/cross-package/assert_runtime_identity.py

python examples/cross-package/assert_stale_locks.py
```

If Catalog changes `SignalDefinition`, update packages in dependency order:

```sh
# 1. Accept and build the owner.
rspyts lock --config examples/cross-package/catalog/rspyts.toml
rspyts build --config examples/cross-package/catalog/rspyts.toml

# 2. Re-resolve, accept, and build the consumer.
rspyts lock --config examples/cross-package/reports/rspyts.toml
rspyts build --config examples/cross-package/reports/rspyts.toml

# 3. Prove both committed contracts are current.
rspyts check --locked \
  --config examples/cross-package/catalog/rspyts.toml
rspyts check --locked \
  --config examples/cross-package/reports/rspyts.toml
```

The result is one authored Rust type, normal host imports, ignored generated
output, and compile-time semantic locks that make contract drift a build
failure. Release tooling still owns Python and npm dependency version ranges.
