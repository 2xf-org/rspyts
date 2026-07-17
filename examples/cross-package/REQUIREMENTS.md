# Cross-package compiler acceptance

This fixture locks in the package-identity behavior implemented by rspyts. The
Evaluation contract links Hardware registrations, but keeps Hardware-owned
definitions as verified imports rather than absorbing them as local exports.

## IR

1. Every type, error, function, resource, and constant definition has an
   immutable Cargo-package owner captured where its macro expands. Do not infer
   ownership from the final cdylib because Cargo dependencies are linked into
   it.
2. Named references are fully qualified as `(owner, id)`. A bare string ID cannot
   distinguish equal Rust item names owned by two packages.
3. Sorted dependency records in the resolved contract contain the owner Cargo
   package, enabled host package identities, exact
   semantic fingerprint, and the referenced definitions needed for validation.
4. Local definitions and imported definitions remain distinct. Imported
   definitions are schema inputs; they are never locally emitted definitions.

The implemented semantic shape is:

```text
DefinitionId { owner: CargoPackageId, id: String }
TypeDef { owner: CargoPackageId, ... }
ImportedPackage { owner: CargoPackageId, types: Vec<TypeDef>, errors: Vec<ErrorDef> }
LockedDependency {
  crate: CargoPackageId,
  python: Option<String>,
  typescript: Option<String>,
  fingerprint: String,
  types: Vec<TypeDef>,
  errors: Vec<ErrorDef>,
}
```

Ownership participates in canonical serialization.

## Configuration

The consuming `evaluation/rspyts.toml` uses this strict dependency mapping:

```toml
[dependencies.hardware]
crate = "cross-package-hardware"
lock = "../hardware/rspyts.lock"
python = "neurovirtual.hardware"
typescript = "@neurovirtual/hardware"
```

The table key is only a local deterministic alias. `crate` is the exact Cargo
package owner embedded in IR. `lock` resolves relative to the consuming config.
The compiler rejects unknown owners, missing mappings for enabled host
targets, fingerprint mismatches, type-shape mismatches, dependency cycles, and
two aliases for one owner.

The config does not accept a path to generated Python or TypeScript source. The dependency
is between released host packages and locked semantic contracts, not between
staging directories.

## Contract resolution and locks

1. Load all linked registrations with their macro-captured owner.
2. Retain only root-owned functions, resources, errors, and constants as local
   exports. A dependency's `define_signal` must never appear in Evaluation.
3. Find foreign named references reachable from local exports and local types.
4. Resolve each foreign owner through `[dependencies]`, read its lock, verify
   the referenced definitions exactly, and reject undeclared transitive use.
5. Write `rspyts.lock` schema 2 with a sorted `dependencies` map containing each
   dependency's exact fingerprint and host identities.
6. Include that sorted dependency data in the consuming package fingerprint so
   a dependency update cannot silently diverge.

The lock is source-controlled. `.rspyts/` remains ignored.

## Python emitter/runtime

- Emit only root-owned model classes.
- For a foreign reference, write a normal absolute import, here
  `from neurovirtual.hardware import SignalDefinition`.
- It is acceptable for `neurovirtual.evaluation.SignalDefinition` to re-export
  that imported object, but it must be the exact same class object as
  `neurovirtual.hardware.SignalDefinition`.
- Generate codecs against the imported type and its verified schema; do not
  synthesize an Evaluation-owned DTO or conversion graph.
- Register only root-owned native functions/resources in Evaluation's PyO3
  module.

## TypeScript emitter/runtime

- Apply the same ownership rule and emit package imports such as
  `import type { SignalDefinition } from "@neurovirtual/hardware"`.
- Never copy the foreign declaration into the consumer package.
- A Python-only consumer does not gain a TypeScript artifact merely because its
  dependency has one.

## Acceptance

These commands must all pass:

```sh
cargo test --manifest-path examples/cross-package/Cargo.toml
cargo run -p rspyts-cli -- lock --config examples/cross-package/hardware/rspyts.toml
cargo run -p rspyts-cli -- lock --config examples/cross-package/evaluation/rspyts.toml
cargo run -p rspyts-cli -- check --locked --config examples/cross-package/hardware/rspyts.toml
cargo run -p rspyts-cli -- check --locked --config examples/cross-package/evaluation/rspyts.toml
python examples/cross-package/assert_identity.py
uv run --with 'pydantic>=2.11' --with 'numpy>=2' \
  python examples/cross-package/assert_runtime_identity.py
python examples/cross-package/assert_stale_locks.py
```

No bridge type, mirrored contract, generated-source dependency, or post-build
rewriter is allowed to make the assertions pass.
