/**
 * Smoke tests for shared declaration provenance across bridged crates.
 *
 * The app's generated code imports `Point` and `Axis` from the
 * shared-types package (via `[typescript.imports]`) instead of re-emitting
 * them. The assertions below prove mutual structural assignability and the
 * live calls prove round-trips; TypeScript does not provide nominal identity
 * for interfaces, so assignability alone cannot prove an import's origin.
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

// Compile-time compatibility guard: the app's parameter and return types are
// mutually assignable with the shared package's exports. This would also hold
// for independently emitted lookalikes, so the generated import itself is the
// declaration-provenance guarantee.
type MutuallyAssignable<A, B> = A extends B ? (B extends A ? true : never) : never;
type _PointCompatibility = MutuallyAssignable<
  Point,
  ReturnType<MultiCrateAppClient["translate"]>
>;
type _AxisCompatibility = MutuallyAssignable<
  Axis,
  Parameters<MultiCrateAppClient["mirror"]>[1]
>;
const importsAreAssignable: _PointCompatibility & _AxisCompatibility = true;

let client: MultiCrateAppClient;

beforeAll(async () => {
  const { instantiate } = await import("rspyts");
  client = createClient(await instantiate(await readFile(WASM_PATH)));
});

describe("shared declaration provenance", () => {
  it("imported declarations remain mutually assignable", () => {
    expect(importsAreAssignable).toBe(true);
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
