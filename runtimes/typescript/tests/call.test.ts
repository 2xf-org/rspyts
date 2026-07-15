import { describe, expect, it } from "vitest";

import {
  boolFromWire,
  boundedIntFromWire,
  bufferFromWire,
  bytesFromWire,
  callDrop,
  callFn,
  enumFromWire,
  f32FromWire,
  floatFromWire,
  i64FromWire,
  i64ToWire,
  jsonFromWire,
  listFromWire,
  mapFromWire,
  nullFromWire,
  objectFromWire,
  stringEnumFromWire,
  stringFromWire,
  tupleFromWire,
  u64FromWire,
  u64ToWire,
  wireResponse,
  writeBytes,
  writeSlice,
  type WireResponse,
} from "../src/call.js";
import {
  BUF_PLACEHOLDER_KEY,
  decodeRequest,
  jsonToWire,
  wireBuffer,
  type BufTypedArray,
  type Dtype,
} from "../src/envelope.js";
import {
  InstancePoisonedError,
  RspytsError,
  RspytsPanicError,
  StaleHandleError,
} from "../src/errors.js";
import { isPoisoned } from "../src/module.js";
import { createFake, runtimeTrap } from "./fake.js";

const EMPTY = new Uint8Array();
const response = <Value>(value: Value, tail: Uint8Array = EMPTY): WireResponse<Value> => ({
  value,
  tail,
});
const placeholder = (off: number, len: number, dt: Dtype | "bytes") => ({
  [BUF_PLACEHOLDER_KEY]: { off, len, dt },
});
const valuesOf = (array: BufTypedArray): Array<number | bigint> =>
  Array.from(array as unknown as ArrayLike<number | bigint>);

describe("schema-directed successful response conversion", () => {
  it("validates exact scalars and 64-bit canonical forms", () => {
    expect(boolFromWire(response(true))).toBe(true);
    expect(stringFromWire(response("ready"))).toBe("ready");
    expect(boundedIntFromWire(response(255), 0, 255)).toBe(255);
    expect(floatFromWire(response(-0))).toBe(0);
    expect(f32FromWire(response(3.4028235e38))).toBe(3.4028235e38);
    expect(i64FromWire(response(i64ToWire(-(1n << 63n))))).toBe(-(1n << 63n));
    expect(u64FromWire(response(u64ToWire((1n << 64n) - 1n)))).toBe((1n << 64n) - 1n);

    expect(() => boolFromWire(response(1))).toThrow(/JSON boolean/);
    expect(() => stringFromWire(response(false))).toThrow(/JSON string/);
    expect(() => boundedIntFromWire(response(1.5), 0, 2)).toThrow(/JSON integer/);
    expect(() => boundedIntFromWire(response(3), 0, 2)).toThrow(/out of range/);
    expect(() => floatFromWire(response(Number.NaN))).toThrow(/finite JSON number/);
    expect(() => f32FromWire(response(1e100))).toThrow(/f32 value out of range/);
    expect(() => i64FromWire(response("01"))).toThrow(/canonical signed/);
    expect(() => u64FromWire(response("18446744073709551616"))).toThrow(/out of range/);
    expect(() => i64ToWire(1 as unknown as bigint)).toThrow(/bigint/);
  });

  it("carries the same explicit tail into every list, tuple, map, object, and enum child", () => {
    const tail = new Uint8Array([9]);
    const list = listFromWire(response([1, 2], tail));
    expect(list.map((item) => item.value)).toEqual([1, 2]);
    expect(list.every((item) => item.tail === tail)).toBe(true);
    expect(tupleFromWire(response(["a", "b"], tail), 2)[1]!.tail).toBe(tail);

    const map = mapFromWire(response({ alpha: 1, __proto__: 2 }, tail));
    expect(Object.getPrototypeOf(map)).toBeNull();
    expect(map["alpha"]).toEqual({ value: 1, tail });

    const object = objectFromWire(response({ required: 1, optional: null }, tail), ["required"], [
      "optional",
    ]);
    expect(object["required"]!.tail).toBe(tail);
    expect(object["optional"]!.value).toBeNull();

    const tagged = enumFromWire(
      response({ kind: "ready", count: 1 }, tail),
      "kind",
      { ready: { required: ["count"] } },
    );
    expect(stringFromWire(tagged["kind"]!)).toBe("ready");
    expect(tagged["count"]!.tail).toBe(tail);

    expect(() => listFromWire(response({}))).toThrow(/JSON array/);
    expect(() => tupleFromWire(response([1]), 2)).toThrow(/length 2/);
    expect(() => mapFromWire(response([]))).toThrow(/JSON object/);
    expect(() => objectFromWire(response({}), ["required"])).toThrow(/required wire field/);
    expect(() => objectFromWire(response({ required: 1, old: 2 }), ["required"]))
      .toThrow(/unexpected wire field/);
    expect(() => enumFromWire(response({ kind: "old" }), "kind", { ready: { required: [] } }))
      .toThrow(/discriminator/);
  });

  it("validates enums, null, transparent Json, and explicit attachments", () => {
    expect(stringEnumFromWire(response("ready"), ["ready", "pending"])).toBe("ready");
    expect(() => stringEnumFromWire(response("old"), ["ready"])).toThrow(/unknown/);
    expect(nullFromWire(response(null))).toBeNull();
    expect(() => nullFromWire(response(false))).toThrow(/expected null/);

    const reserved = { [BUF_PLACEHOLDER_KEY]: { off: 999, len: 1, dt: "u8" } };
    expect(jsonFromWire(response(reserved, new Uint8Array([4])))).toEqual(reserved);
    const cyclic: Record<string, unknown> = {};
    cyclic["self"] = cyclic;
    expect(() => jsonFromWire(response(cyclic))).toThrow(/cycles/);
    expect(() => jsonFromWire(response({ value: Number.MAX_SAFE_INTEGER + 1 }))).toThrow(
      /safe integers/,
    );

    const numeric = new Float64Array([1.5, -2.5]);
    expect(
      valuesOf(
        bufferFromWire(
          response(placeholder(0, 2, "f64"), new Uint8Array(numeric.buffer)),
          "f64",
        ),
      ),
    ).toEqual([1.5, -2.5]);
    expect(
      Array.from(bytesFromWire(response(placeholder(0, 2, "bytes"), new Uint8Array([1, 2])))),
    ).toEqual([1, 2]);
    expect(() => bufferFromWire(response(placeholder(0, 1, "u8"), new Uint8Array([1])), "i8"))
      .toThrow(/expected a i8.*got u8/);
  });

  it("builds explicit empty-tail context for attachment-free error conversion", () => {
    const context = wireResponse({ factor: 1.5 });
    expect(context.tail.byteLength).toBe(0);
    expect(floatFromWire(objectFromWire(context, ["factor"])["factor"]!)).toBe(1.5);
  });
});

describe("single callFn path", () => {
  it("sends envelope-framed arguments, schema markers, and a leading exact handle", () => {
    const fake = createFake();
    let seen: unknown;
    let handle: unknown;
    fake.setExport("rspyts_cls__Stats__push", (...raw: Array<number | bigint>) => {
      handle = raw[0];
      const [argsPtr, argsLen] = raw.slice(1) as number[];
      const request = decodeRequest(fake.readBytes(argsPtr!, argsLen!));
      const value = request.value as Record<string, unknown>;
      seen = {
        json: value["json"],
        samples: bufferFromWire(response(value["samples"], request.tail), "i32"),
      };
      return fake.putEnvelope(0, null);
    });
    const json = { [BUF_PLACEHOLDER_KEY]: { off: 0, len: 9, dt: "u8" } };
    const result = callFn(
      fake.mod,
      "rspyts_cls__Stats__push",
      { json: jsonToWire(json), samples: wireBuffer(new Int32Array([-2, 7]), "i32") },
      [],
      (1n << 40n) + 7n,
    );
    expect(handle).toBe((1n << 40n) + 7n);
    expect((seen as { json: unknown }).json).toEqual(json);
    expect(Array.from((seen as { samples: Int32Array }).samples)).toEqual([-2, 7]);
    expect(result).toEqual({ value: null, tail: new Uint8Array() });
    fake.assertAllFreedOnce();
  });

  it("rejects handles outside the shared nonzero safe-integer domain", () => {
    const fake = createFake();
    fake.setExport("rspyts_cls__Thing__method", () => fake.putEnvelope(0, null));
    expect(() => callFn(fake.mod, "rspyts_cls__Thing__method", {}, [], 0n)).toThrow(
      /between 1 and Number.MAX_SAFE_INTEGER/,
    );
    expect(() =>
      callFn(fake.mod, "rspyts_cls__Thing__method", {}, [], BigInt(Number.MAX_SAFE_INTEGER) + 1n)
    ).toThrow(/between 1 and Number.MAX_SAFE_INTEGER/);
    expect(() => callDrop(fake.mod, "rspyts_cls__Thing__drop", 0n)).toThrow(
      /between 1 and Number.MAX_SAFE_INTEGER/,
    );
  });

  it("passes all slice pairs in order with natural alignment", () => {
    const fake = createFake();
    fake.misalign(3);
    let seen: number[] | undefined;
    fake.setExport("rspyts_fn__mix", (...raw: number[]) => {
      const [, , aPtr, aLen, bPtr, bLen] = raw;
      expect(aPtr! % 2).toBe(0);
      expect(bPtr! % 8).toBe(0);
      seen = [
        ...new Int16Array(fake.memory.buffer, aPtr!, aLen!),
        ...new Float64Array(fake.memory.buffer, bPtr!, bLen!),
      ];
      return fake.putEnvelope(0, { done: true });
    });
    const result = callFn(fake.mod, "rspyts_fn__mix", {}, [
      { data: new Int16Array([-1, 300]), dt: "i16" },
      { data: new Float64Array([1.5]), dt: "f64" },
    ]);
    expect(seen).toEqual([-1, 300, 1.5]);
    expect(result.value).toEqual({ done: true });
    fake.assertAllFreedOnce();
  });

  it("returns explicit attachment context copied before module memory is freed", () => {
    const fake = createFake({ zeroOnFree: true });
    const data = new Float64Array([1, 2, 3]);
    fake.setExport("rspyts_fn__spectrum", () =>
      fake.putEnvelope(
        0,
        { bins: placeholder(0, 3, "f64"), peak: 2 },
        new Uint8Array(data.buffer),
      ),
    );
    const raw = callFn(fake.mod, "rspyts_fn__spectrum", {});
    const object = objectFromWire(raw, ["bins", "peak"]);
    const bins = bufferFromWire(object["bins"]!, "f64") as Float64Array;
    expect(Array.from(bins)).toEqual([1, 2, 3]);
    expect(boundedIntFromWire(object["peak"]!, 0, 10)).toBe(2);
    expect(Array.from(raw.tail)).toEqual(Array.from(new Uint8Array(data.buffer)));
    fake.assertAllFreedOnce();
  });

  it("uses only call-scoped error registries and preserves bridge-owned stale handles", () => {
    class ScopedError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "scoped", data);
      }
    }
    const fake = createFake();
    fake.setExport("rspyts_fn__fails", () =>
      fake.putEnvelope(1, { code: "scoped", message: "only here", data: { value: 1 } }),
    );
    expect(() =>
      callFn(fake.mod, "rspyts_fn__fails", {}, [], undefined, { scoped: ScopedError }),
    ).toThrow(ScopedError);

    const stale = createFake();
    stale.setExport("rspyts_fn__stale", () =>
      stale.putEnvelope(1, { code: "staleHandle", message: "gone" }),
    );
    expect(() => callFn(stale.mod, "rspyts_fn__stale", {})).toThrow(StaleHandleError);
  });

  it("maps panic envelopes and rejects error attachment tails", () => {
    const panic = createFake();
    panic.setExport("rspyts_fn__panic", () =>
      panic.putEnvelope(2, { code: "panic", message: "kaboom" }),
    );
    expect(() => callFn(panic.mod, "rspyts_fn__panic", {})).toThrow(RspytsPanicError);

    const invalid = createFake();
    invalid.setExport("rspyts_fn__invalid", () =>
      invalid.putEnvelope(1, { code: "x", message: "bad" }, new Uint8Array([1])),
    );
    expect(() => callFn(invalid.mod, "rspyts_fn__invalid", {})).toThrow(/attachment bytes/);
  });

  it("survives memory growth during every allocation and the bridged call", () => {
    const fake = createFake({ growOnAlloc: true });
    let args: unknown;
    fake.setExport(
      "rspyts_fn__grow",
      (argsPtr: number, argsLen: number, slicePtr: number, sliceLen: number) => {
        args = decodeRequest(fake.readBytes(argsPtr, argsLen)).value;
        expect(Array.from(new Float64Array(fake.memory.buffer, slicePtr, sliceLen))).toEqual([
          4.25,
          -8.5,
        ]);
        fake.memory.grow(1);
        const ptr = fake.putEnvelope(0, { echoed: true });
        fake.memory.grow(1);
        return ptr;
      },
    );
    expect(
      callFn(fake.mod, "rspyts_fn__grow", { n: 42 }, [
        { data: new Float64Array([4.25, -8.5]), dt: "f64" },
      ]).value,
    ).toEqual({ echoed: true });
    expect(args).toEqual({ n: 42 });
    fake.assertAllFreedOnce();
  });
});

describe("ownership, bounds, and trap lifecycle", () => {
  it("frees request, slices, and response exactly once on success and errors", () => {
    const success = createFake();
    success.setExport("rspyts_fn__work", () => success.putEnvelope(0, null));
    callFn(success.mod, "rspyts_fn__work", { a: 1 }, [
      { data: new Uint8Array([1]), dt: "u8" },
      { data: new Float32Array([2]), dt: "f32" },
    ]);
    success.assertAllFreedOnce();
    expect(success.freed).toHaveLength(4);

    const failure = createFake();
    failure.setExport("rspyts_fn__work", () =>
      failure.putEnvelope(1, { code: "no", message: "no" }),
    );
    expect(() => callFn(failure.mod, "rspyts_fn__work", { a: 1 })).toThrow(RspytsError);
    failure.assertAllFreedOnce();
    expect(failure.freed).toHaveLength(2);
  });

  it("rejects bad pointers and out-of-bounds envelopes", () => {
    const nonPointer = createFake();
    nonPointer.setExport("rspyts_fn__bad", () => "bad");
    expect(() => callFn(nonPointer.mod, "rspyts_fn__bad", {})).toThrow(/non-pointer/);
    nonPointer.assertAllFreedOnce();

    const bounds = createFake();
    bounds.setExport("rspyts_fn__bad", () => bounds.memory.buffer.byteLength - 4);
    expect(() => callFn(bounds.mod, "rspyts_fn__bad", {})).toThrow(/response header range/);
    expect(isPoisoned(bounds.mod)).toBe(true);
  });

  it("poisons on WebAssembly traps and rejects later calls", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__trap", () => {
      throw runtimeTrap();
    });
    expect(() => callFn(fake.mod, "rspyts_fn__trap", {})).toThrow(WebAssembly.RuntimeError);
    expect(isPoisoned(fake.mod)).toBe(true);
    expect(() => callFn(fake.mod, "rspyts_fn__trap", {})).toThrow(InstancePoisonedError);
    expect(fake.freed).toHaveLength(0);
  });

  it("does not poison ordinary JavaScript errors and still frees request allocations", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__error", () => {
      throw new TypeError("fixture error");
    });
    expect(() => callFn(fake.mod, "rspyts_fn__error", {})).toThrow("fixture error");
    expect(isPoisoned(fake.mod)).toBe(false);
    fake.assertAllFreedOnce();
  });

  it("swallows drop traps, poisons the module, and skips later drops", () => {
    const fake = createFake();
    let calls = 0;
    fake.setExport("rspyts_cls__Thing__drop", () => {
      calls += 1;
      throw runtimeTrap("drop");
    });
    expect(() => callDrop(fake.mod, "rspyts_cls__Thing__drop", 1n)).not.toThrow();
    callDrop(fake.mod, "rspyts_cls__Thing__drop", 2n);
    expect(calls).toBe(1);
    expect(isPoisoned(fake.mod)).toBe(true);
  });
});

describe("low-level request allocation helpers", () => {
  it("copies bytes after allocation and aligns slices while retaining exact free pairs", () => {
    const fake = createFake({ growOnAlloc: true });
    const input = new Uint8Array([1, 2, 3]);
    const bytes = writeBytes(fake.mod, input);
    input.fill(9);
    expect(Array.from(fake.readBytes(bytes.ptr, bytes.len))).toEqual([1, 2, 3]);

    const slice = writeSlice(fake.mod, { data: new Float64Array([42]), dt: "f64" });
    expect(slice.aligned % 8).toBe(0);
    expect(slice.len).toBe(16);
    expect(Array.from(new Float64Array(fake.memory.buffer, slice.aligned, 1))).toEqual([42]);
  });

  it("rejects a typed-array constructor that does not match the declared dtype", () => {
    const fake = createFake();
    expect(() => writeSlice(fake.mod, { data: new Uint8Array([1]), dt: "f64" })).toThrow(
      /requires a Float64Array/,
    );
  });
});
