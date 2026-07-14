/**
 * Smoke tests proving shared type identity across bridged crates.
 *
 * The app's generated code imports `Point` and `Axis` from the
 * shared-types package (via `[typescript.imports]`) instead of re-emitting
 * them, so the type this suite pulls from "shared-types-example" is —
 * not just resembles — the type the app client accepts and returns.
 *
 * Requires the WASM build:
 *   rspyts build --config examples/multi-crate/app/rspyts.toml
 */

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";
import type { Axis, Point } from "shared-types-example";
// Imported from ../src, not ../src/generated: the package's index.ts
// re-exports the generated surface, so consumers never spell "generated".
import { createClient, type MultiCrateAppClient } from "../src";

const WASM_PATH = fileURLToPath(
  new URL(
    "../../../../../target/rspyts/wasm32-unknown-unknown/debug/multi_crate_app.wasm",
    import.meta.url,
  ),
);

// Compile-time proof of identity: the app's parameter and return types are
// mutually assignable with the shared package's exports. `AssertSame`
// resolves to `true` only for identical (mutually assignable) types.
type AssertSame<A, B> = A extends B ? (B extends A ? true : never) : never;
type _PointIdentity = AssertSame<Point, ReturnType<MultiCrateAppClient["translate"]>>;
type _AxisIdentity = AssertSame<Axis, Parameters<MultiCrateAppClient["mirror"]>[1]>;
const identityHolds: _PointIdentity & _AxisIdentity = true;

let client: MultiCrateAppClient;

beforeAll(async () => {
  const { instantiate } = await import("rspyts");
  client = createClient(await instantiate(await readFile(WASM_PATH)));
});

describe("shared identity", () => {
  it("type-level identity holds", () => {
    expect(identityHolds).toBe(true);
  });

  it("translate accepts and returns the shared Point", () => {
    const point: Point = { x: 1, y: 2 };
    const moved: Point = client.translate(point, 3, 4);
    expect(moved).toEqual({ x: 4, y: 6 });
  });

  it("mirror accepts the shared Axis literals", () => {
    const flipped: Point = client.mirror({ x: 1, y: 2 }, "vertical");
    expect(flipped).toEqual({ x: -1, y: 2 });
    expect(client.mirror({ x: 1, y: 2 }, "horizontal")).toEqual({ x: 1, y: -2 });
  });

  it("results round-trip back into app functions", () => {
    const point: Point = { x: 2, y: -3 };
    const thereAndBack = client.translate(client.mirror(point, "horizontal"), 0, -3);
    expect(thereAndBack).toEqual({ x: 2, y: 0 });
  });
});
