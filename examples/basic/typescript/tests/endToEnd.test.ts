/**
 * End-to-end tests: TypeScript → WASM → envelope → typed results.
 *
 * Requires the WASM build:
 *   cargo build -p basic-example --target wasm32-unknown-unknown
 */

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";
import { RspytsError, RspytsPanicError, StaleHandleError } from "rspyts";
// Imported from ../src, not ../src/generated: the package's index.ts
// re-exports the generated surface, so consumers never spell "generated".
import {
  createClient,
  MAX_WINDOW,
  ROUNDING_MODES,
  type BasicExampleClient,
  type Rounding,
} from "../src";

const WASM_PATH = fileURLToPath(
  new URL(
    "../../../../target/wasm32-unknown-unknown/debug/basic_example.wasm",
    import.meta.url,
  ),
);

let client: BasicExampleClient;

beforeAll(async () => {
  const { instantiate } = await import("rspyts");
  client = createClient(await instantiate(await readFile(WASM_PATH)));
});

describe("summarize", () => {
  it("computes stats", () => {
    const summary = client.summarize(new Float64Array([2, 4, 6]), "demo");
    expect(summary).toMatchObject({ itemCount: 3, total: 12, average: 4, label: "demo" });
  });

  it("accepts a null label", () => {
    expect(client.summarize(new Float64Array([1]), null).label).toBeNull();
  });

  it("throws a typed error for empty input", () => {
    expect(() => client.summarize(new Float64Array(0), null)).toThrowError(
      expect.objectContaining({ code: "emptyInput" }),
    );
  });

  it("rejects more than MAX_WINDOW values", () => {
    expect(client.summarize(new Float64Array(MAX_WINDOW).fill(1), null).itemCount).toBe(MAX_WINDOW);
    expect(() => client.summarize(new Float64Array(MAX_WINDOW + 1).fill(1), null)).toThrowError(
      expect.objectContaining({ code: "tooManyValues", data: { count: MAX_WINDOW + 1 } }),
    );
  });
});

describe("constants", () => {
  it("are importable with their captured values", () => {
    expect(MAX_WINDOW).toBe(1024);
    expect(ROUNDING_MODES).toEqual(["up", "down", "nearest", "halfEven"]);
  });

  it("carry literal types via `as const`", () => {
    // MAX_WINDOW is the literal type 1024 and ROUNDING_MODES a readonly
    // tuple of string literals — each element is assignable to Rounding.
    const window: 1024 = MAX_WINDOW;
    const nearest: "nearest" = ROUNDING_MODES[2];
    const mode: Rounding = ROUNDING_MODES[3];
    expect(client.roundValue(2.5, mode)).toBe(2);
    expect(client.roundValue(2.4, nearest)).toBe(2);
    expect(window).toBe(1024);
  });
});

describe("annotate", () => {
  it("round-trips schemaless JSON objects untouched", () => {
    const metadata = { source: "sensor", nested: { tags: ["a", "b"], rev: 2 }, empty: null };
    expect(client.annotate(2.5, metadata)).toEqual({ ...metadata, value: 2.5 });
  });

  it("wraps non-object metadata", () => {
    expect(client.annotate(1, [1, "two", false])).toEqual({ input: [1, "two", false], value: 1 });
    expect(client.annotate(0.5, "note")).toEqual({ input: "note", value: 0.5 });
  });
});

describe("scale", () => {
  it("returns a Float64Array buffer", () => {
    const out = client.scale(new Float64Array([1, 2, 3]), 2);
    expect(out).toBeInstanceOf(Float64Array);
    expect(Array.from(out)).toEqual([2, 4, 6]);
  });

  it("round-trips large buffers", () => {
    const values = new Float64Array(100_000).map((_, i) => i / 1000);
    const out = client.scale(values, 3);
    expect(out.length).toBe(values.length);
    expect(out[9000]).toBeCloseTo(27);
  });

  it("carries error data for zero factors", () => {
    try {
      client.scale(new Float64Array([1]), 0);
      expect.unreachable();
    } catch (error) {
      expect(error).toBeInstanceOf(RspytsError);
      expect((error as RspytsError).code).toBe("zeroFactor");
      expect((error as RspytsError).data).toEqual({ factor: 0 });
    }
  });

  it("rejects non-finite scalars loudly", () => {
    // JSON.stringify turns Infinity into null, which Rust's deserializer
    // rejects — a loud invalidArgs error, never silent corruption.
    expect(() => client.scale(new Float64Array([1]), Number.POSITIVE_INFINITY)).toThrowError(
      expect.objectContaining({ code: "invalidArgs" }),
    );
  });
});

describe("roundValue", () => {
  it("string enum parameters cross as literals", () => {
    expect(client.roundValue(2.4, "up")).toBe(3);
    expect(client.roundValue(2.6, "down")).toBe(2);
    expect(client.roundValue(2.5, "nearest")).toBe(3);
  });

  it("halfEven breaks ties toward even", () => {
    // Variant wire values are camelCase — the convention IS the contract.
    expect(client.roundValue(2.5, "halfEven")).toBe(2);
    expect(client.roundValue(3.5, "halfEven")).toBe(4);
    // @ts-expect-error -- the snake_case spelling is not a Rounding.
    expect(() => client.roundValue(2.5, "half_even")).toThrowError(
      expect.objectContaining({ code: "invalidArgs" }),
    );
  });
});

describe("parseNumber", () => {
  it("discriminated union narrows on type", () => {
    const parsed = client.parseNumber("42");
    expect(parsed.type).toBe("integer");
    if (parsed.type === "integer") {
      expect(parsed.value).toBe(42);
    }
    const decimal = client.parseNumber("3.5");
    expect(decimal.type).toBe("decimal");
  });

  it("carries the offending text in error data", () => {
    try {
      client.parseNumber("abc");
      expect.unreachable();
    } catch (error) {
      expect((error as RspytsError).code).toBe("notANumber");
      expect((error as RspytsError).data).toEqual({ text: "abc" });
    }
  });
});

describe("Counter", () => {
  it("factory returns a live handle", () => {
    const counter = client.Counter.startingAtZero("fresh");
    expect(counter.currentValue()).toBe(0);
    expect(counter.increment(3)).toBe(3);
    expect(counter.label()).toBe("fresh");
    counter.free();
    expect(() => counter.currentValue()).toThrowError(StaleHandleError);
  });

  it("factory and constructor instances are independent", () => {
    const constructed = new client.Counter(10, "ctor");
    const made = client.Counter.startingAtZero("factory");
    constructed.increment(1);
    made.increment(2);
    expect(constructed.currentValue()).toBe(11);
    expect(made.currentValue()).toBe(2);
    constructed.free();
    made.free();
  });

  it("static helpers need no instance", () => {
    expect(client.Counter.defaultLabel()).toBe("unnamed");
  });

  it("full lifecycle", () => {
    const counter = new client.Counter(10, "bench");
    expect(counter.increment(5)).toBe(15);
    expect(counter.addParsed("7")).toBe(22);
    expect(counter.currentValue()).toBe(22);
    expect(counter.label()).toBe("bench");
    counter.reset();
    expect(counter.currentValue()).toBe(10);
    counter.free();
  });

  it("method errors are typed", () => {
    const counter = new client.Counter(0, "err");
    expect(() => counter.addParsed("1.5")).toThrowError(
      expect.objectContaining({ code: "notANumber" }),
    );
    expect(counter.currentValue()).toBe(0);
    counter.free();
  });

  it("throws StaleHandleError after free", () => {
    const counter = new client.Counter(0, "stale");
    counter.free();
    expect(() => counter.increment(1)).toThrowError(StaleHandleError);
  });

  it("free is idempotent", () => {
    const counter = new client.Counter(0, "idem");
    counter.free();
    counter.free();
  });

  it("instances are independent", () => {
    const a = new client.Counter(0, "a");
    const b = new client.Counter(100, "b");
    a.increment(1);
    b.increment(1);
    expect(a.currentValue()).toBe(1);
    expect(b.currentValue()).toBe(101);
    a.free();
    b.free();
  });
});

describe("wire format", () => {
  it("uses camelCase field names end to end", () => {
    const summary = client.summarize(new Float64Array([1]), null);
    expect(Object.keys(summary)).toContain("itemCount");
  });
});

describe("target scoping", () => {
  it("omits python-only functions from the client", () => {
    // `load_values` is `#[bridge(target = "python")]`: the Rust shim exists,
    // but the TypeScript projection never surfaces it.
    expect(Object.keys(client)).not.toContain("loadValues");
    // @ts-expect-error -- loadValues is not part of BasicExampleClient.
    expect(client.loadValues).toBeUndefined();
  });
});

describe("unicode", () => {
  it("round-trips labels exactly", () => {
    for (const label of ["📈🚀", "统计概要", "ملخص عربي", "mix 数🚀 عرب"]) {
      expect(client.summarize(new Float64Array([1]), label).label).toBe(label);
    }
  });

  it("preserves unicode text exactly in error data", () => {
    const garbage = "١٢٣ ≠ 数字 🚀";
    try {
      client.parseNumber(garbage);
      expect.unreachable();
    } catch (error) {
      expect((error as RspytsError).code).toBe("notANumber");
      expect((error as RspytsError).data).toEqual({ text: garbage });
    }
  });
});

describe("boundaries", () => {
  it("carries i32 extremes through Counter without corruption", () => {
    const max = new client.Counter(2_147_483_647, "max");
    expect(max.currentValue()).toBe(2_147_483_647);
    max.free();
    const walk = new client.Counter(0, "walk");
    expect(walk.increment(2_147_483_647)).toBe(2_147_483_647);
    walk.reset();
    expect(walk.increment(-2_147_483_648)).toBe(-2_147_483_648);
    walk.free();
  });

  it("distinguishes an empty-string label from null", () => {
    expect(client.summarize(new Float64Array([1]), "").label).toBe("");
    expect(client.summarize(new Float64Array([1]), null).label).toBeNull();
  });
});

describe("bulk buffers", () => {
  it("scales an empty buffer to an empty buffer", () => {
    const out = client.scale(new Float64Array(0), 2);
    expect(out).toBeInstanceOf(Float64Array);
    expect(out.length).toBe(0);
  });

  it("round-trips a million-element buffer with exact spot values", () => {
    const values = new Float64Array(1_000_000);
    for (let i = 0; i < values.length; i++) {
      values[i] = i;
    }
    const out = client.scale(values, 2);
    expect(out.length).toBe(1_000_000);
    expect(out[0]).toBe(0);
    expect(out[123_456]).toBe(246_912);
    expect(out[999_999]).toBe(1_999_998);
  });

  it("carries f64 specials through buffers", () => {
    // JSON positions reject NaN/Infinity, but slice and Buf payloads are
    // raw bytes — specials cross intact and scale correctly.
    const out = client.scale(new Float64Array([NaN, Infinity, -Infinity, 0]), 2);
    expect(out[0]).toBeNaN();
    expect(out[1]).toBe(Infinity);
    expect(out[2]).toBe(-Infinity);
    expect(out[3]).toBe(0);
  });
});

describe("handle lifecycle", () => {
  it("supports 200 simultaneous counters with interleaved ops", () => {
    const counters = Array.from({ length: 200 }, (_, i) => new client.Counter(i, `c${i}`));
    for (const counter of counters) {
      counter.increment(1_000);
    }
    for (let i = 0; i < counters.length; i += 2) {
      counters[i].reset();
    }
    counters.forEach((counter, i) => {
      expect(counter.currentValue()).toBe(i % 2 === 0 ? i : i + 1_000);
      expect(counter.label()).toBe(`c${i}`);
      counter.free();
    });
  });

  it("every method throws StaleHandleError after free", () => {
    const counter = new client.Counter(0, "stale");
    counter.free();
    expect(() => counter.increment(1)).toThrowError(StaleHandleError);
    expect(() => counter.addParsed("1")).toThrowError(StaleHandleError);
    expect(() => counter.currentValue()).toThrowError(StaleHandleError);
    expect(() => counter.label()).toThrowError(StaleHandleError);
    expect(() => counter.reset()).toThrowError(StaleHandleError);
  });
});

// Keep last: on wasm32-unknown-unknown a Rust panic becomes a trap, not a
// status-2 envelope (see docs/design/abi.md §9), and the instance may not
// be reusable afterwards.
describe("panics", () => {
  it("a panic traps the instance instead of returning garbage", () => {
    expect(() => client.simulatePanic()).toThrowError();
  });
});

// Type-level assertion (compile-time only).
type _AssertPanicIsError = RspytsPanicError extends Error ? true : never;
