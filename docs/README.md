# Documentation

rspyts lets a Rust crate expose one typed API to Python and TypeScript.

Start here:

- [Quickstart](introduction/quickstart.md) — build a small bridge and call it.
- [How rspyts works](introduction/how-rspyts-works.md) — the idea in five minutes.
- [Python](python.md) — generated models, native loading, buffers, errors, and handles.
- [TypeScript](typescript.md) — generated clients, WebAssembly, buffers, errors, and disposal.

The contract is written down here:

- [0.4 direct Rust API compilation](design/v0.4.md) — draft architecture for
  the breaking next release.
- [0.4 delivery plan](design/v0.4-delivery.md) — phased implementation and
  Cloud acceptance gates.
- [Type system](design/type-system.md) — what may cross the boundary.
- [ABI](design/abi.md) — symbols, memory, envelopes, attachments, and handles.
- [Code generation](design/codegen.md) — configuration, generated files, and drift checks.
- [Decisions](design/decisions.md) — why the project has these limits.

For contributors:

- [Architecture](architecture.md) — where each piece lives.
- [Releasing](releasing.md) — the complete release gate and registry order.
