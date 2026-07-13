import { describe, expect, it } from "vitest";

import { callDrop, callFn, writeBytes, writeSlice } from "../src/call.js";
import type { BufTypedArray, Dtype } from "../src/envelope.js";
import { RspytsError, RspytsPanicError, registerError } from "../src/errors.js";
import { createFake } from "./fake.js";

const utf8 = new TextDecoder();

interface SliceCase {
  dt: Dtype;
  bytes: number;
  make: (values: number[]) => BufTypedArray;
  view: (buffer: ArrayBufferLike, byteOffset: number, length: number) => BufTypedArray;
  values: number[];
}

/** One slice argument per wire dtype, with values exact in every type. */
const SLICE_CASES: SliceCase[] = [
  {
    dt: "u8",
    bytes: 1,
    make: (v) => new Uint8Array(v),
    view: (b, o, l) => new Uint8Array(b, o, l),
    values: [0, 127, 255],
  },
  {
    dt: "i16",
    bytes: 2,
    make: (v) => new Int16Array(v),
    view: (b, o, l) => new Int16Array(b, o, l),
    values: [-32768, -1, 32767],
  },
  {
    dt: "i32",
    bytes: 4,
    make: (v) => new Int32Array(v),
    view: (b, o, l) => new Int32Array(b, o, l),
    values: [-2147483648, 0, 2147483647],
  },
  {
    dt: "f32",
    bytes: 4,
    make: (v) => new Float32Array(v),
    view: (b, o, l) => new Float32Array(b, o, l),
    values: [-0.5, 1.25, 1024],
  },
  {
    dt: "f64",
    bytes: 8,
    make: (v) => new Float64Array(v),
    view: (b, o, l) => new Float64Array(b, o, l),
    values: [Math.PI, -1e300, 0.1],
  },
];

describe("callFn argument encoding", () => {
  it("sends the two-byte '{}' payload for empty args", () => {
    const fake = createFake();
    let seen: string | undefined;
    fake.setExport("rspyts_fn__ping", (argsPtr: number, argsLen: number) => {
      seen = utf8.decode(fake.readBytes(argsPtr, argsLen));
      return fake.putEnvelope(0, null);
    });

    const result = callFn(fake.mod, "rspyts_fn__ping", {});

    expect(seen).toBe("{}");
    expect(result).toBeNull();
  });

  it("encodes plain args as a JSON object", () => {
    const fake = createFake();
    let seen: unknown;
    fake.setExport("rspyts_fn__configure", (argsPtr: number, argsLen: number) => {
      seen = JSON.parse(utf8.decode(fake.readBytes(argsPtr, argsLen)));
      return fake.putEnvelope(0, "ok");
    });

    const args = { sampleRate: 44100, params: { minDurationS: 0.5, threshold: null } };
    const result = callFn(fake.mod, "rspyts_fn__configure", args);

    expect(seen).toEqual(args);
    expect(result).toBe("ok");
  });

  it("passes slices as naturally-aligned (ptr, element count) pairs", () => {
    const fake = createFake();
    let captured: { ptr: number; count: number; values: number[] } | undefined;
    fake.setExport(
      "rspyts_fn__analyze",
      (_argsPtr: number, _argsLen: number, slicePtr: number, sliceLen: number) => {
        captured = {
          ptr: slicePtr,
          count: sliceLen,
          values: Array.from(new Float64Array(fake.memory.buffer, slicePtr, sliceLen)),
        };
        return fake.putEnvelope(0, null);
      },
    );

    // 15 bytes of args JSON leave the bump allocator misaligned, so the
    // slice allocation must actually round up.
    callFn(fake.mod, "rspyts_fn__analyze", { threshold: 1 }, [
      { data: new Float64Array([1.5, -2.5, 3.25]), dt: "f64" },
    ]);

    expect(captured).toBeDefined();
    expect(captured!.ptr % 8).toBe(0);
    expect(captured!.count).toBe(3);
    expect(captured!.values).toEqual([1.5, -2.5, 3.25]);
  });

  it("appends multiple slice pairs in declaration order", () => {
    const fake = createFake();
    let received: Array<number | bigint> | undefined;
    fake.setExport("rspyts_fn__mix", (...args: Array<number | bigint>) => {
      received = args;
      const [, , aPtr, aLen, bPtr, bLen] = args as number[];
      expect(Array.from(new Uint8Array(fake.memory.buffer, aPtr!, aLen!))).toEqual([7, 8]);
      expect(bPtr! % 2).toBe(0);
      expect(Array.from(new Int16Array(fake.memory.buffer, bPtr!, bLen!))).toEqual([-1, 300]);
      return fake.putEnvelope(0, null);
    });

    callFn(fake.mod, "rspyts_fn__mix", {}, [
      { data: new Uint8Array([7, 8]), dt: "u8" },
      { data: new Int16Array([-1, 300]), dt: "i16" },
    ]);

    // [argsPtr, argsLen, s1Ptr, s1Len, s2Ptr, s2Len]
    expect(received).toHaveLength(6);
  });

  it("aligns the slice pointer to the natural alignment of every dtype", () => {
    for (const { dt, bytes, make, view, values } of SLICE_CASES) {
      const fake = createFake();
      fake.misalign(3); // knock the bump pointer off every alignment > 1
      let observed: { ptr: number; values: number[] } | undefined;
      fake.setExport(
        "rspyts_fn__take",
        (_argsPtr: number, _argsLen: number, slicePtr: number, sliceLen: number) => {
          observed = {
            ptr: slicePtr,
            values: Array.from(view(fake.memory.buffer, slicePtr, sliceLen)),
          };
          return fake.putEnvelope(0, null);
        },
      );

      callFn(fake.mod, "rspyts_fn__take", {}, [{ data: make(values), dt }]);

      expect(observed, dt).toBeDefined();
      expect(observed!.ptr % bytes, dt).toBe(0);
      expect(observed!.values, dt).toEqual(Array.from(make(values)));
      fake.assertAllFreedOnce();
    }
  });

  it("passes a zero-length slice as an in-bounds pointer with count 0", () => {
    const fake = createFake();
    let observed: { ptr: number; count: number } | undefined;
    fake.setExport(
      "rspyts_fn__empty",
      (_argsPtr: number, _argsLen: number, slicePtr: number, sliceLen: number) => {
        observed = { ptr: slicePtr, count: sliceLen };
        return fake.putEnvelope(0, null);
      },
    );

    callFn(fake.mod, "rspyts_fn__empty", {}, [{ data: new Float64Array(0), dt: "f64" }]);

    expect(observed).toBeDefined();
    expect(observed!.count).toBe(0);
    expect(observed!.ptr % 8).toBe(0);
    fake.assertAllFreedOnce();
  });

  it("passes the handle as a leading bigint", () => {
    const fake = createFake();
    let receivedHandle: unknown;
    let argCount = 0;
    fake.setExport("rspyts_cls__Stats__push", (...args: Array<number | bigint>) => {
      receivedHandle = args[0];
      argCount = args.length;
      return fake.putEnvelope(0, null);
    });

    callFn(fake.mod, "rspyts_cls__Stats__push", {}, [], 7n);

    expect(typeof receivedHandle).toBe("bigint");
    expect(receivedHandle).toBe(7n);
    expect(argCount).toBe(3); // handle, argsPtr, argsLen
  });

  it("passes handles above 2^32 through as exact bigints", () => {
    const fake = createFake();
    const handle = (1n << 33n) + 12345n; // needs all 64 bits, not a safe f64 round trip zone
    let receivedHandle: unknown;
    fake.setExport("rspyts_cls__Stats__push", (...args: Array<number | bigint>) => {
      receivedHandle = args[0];
      return fake.putEnvelope(0, null);
    });

    callFn(fake.mod, "rspyts_cls__Stats__push", {}, [], handle);

    expect(typeof receivedHandle).toBe("bigint");
    expect(receivedHandle).toBe(handle);
  });

  it("normalizes null and undefined args to the two-byte '{}' payload", () => {
    for (const args of [null, undefined]) {
      const fake = createFake();
      let seen: string | undefined;
      fake.setExport("rspyts_fn__ping", (argsPtr: number, argsLen: number) => {
        seen = utf8.decode(fake.readBytes(argsPtr, argsLen));
        return fake.putEnvelope(0, null);
      });

      callFn(fake.mod, "rspyts_fn__ping", args);

      expect(seen, String(args)).toBe("{}");
      fake.assertAllFreedOnce();
    }
  });

  it("throws on an unknown export symbol", () => {
    const fake = createFake();
    expect(() => callFn(fake.mod, "rspyts_fn__nope", {})).toThrow(/no export "rspyts_fn__nope"/);
  });

  it("rejects a non-numeric pointer from rspyts_alloc", () => {
    const fake = createFake();
    fake.setExport("rspyts_alloc", () => "bogus");
    fake.setExport("rspyts_fn__x", () => 0);
    expect(() => callFn(fake.mod, "rspyts_fn__x", {})).toThrow(/non-numeric pointer/);
  });
});

describe("callFn result handling", () => {
  it("replaces buf placeholders with typed-array copies", () => {
    const fake = createFake();
    const data = new Float64Array([1, 2, 3]);
    fake.setExport("rspyts_fn__spectrum", () =>
      fake.putEnvelope(
        0,
        { bins: { __rspyts_buf__: { off: 0, len: 3, dt: "f64" } }, peak: 2 },
        new Uint8Array(data.buffer),
      ),
    );

    const result = callFn(fake.mod, "rspyts_fn__spectrum", {}) as {
      bins: Float64Array;
      peak: number;
    };

    expect(result.bins).toBeInstanceOf(Float64Array);
    expect(Array.from(result.bins)).toEqual([1, 2, 3]);
    expect(result.peak).toBe(2);
    // The envelope was freed; the returned array must be a copy that
    // survives arbitrary later mutation of linear memory.
    new Uint8Array(fake.memory.buffer).fill(0xff);
    expect(Array.from(result.bins)).toEqual([1, 2, 3]);
  });

  it("throws registered error classes on status 1", () => {
    class WindowTooLargeError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "callTestWindowTooLarge", data);
      }
    }
    registerError("callTestWindowTooLarge", WindowTooLargeError);

    const fake = createFake();
    fake.setExport("rspyts_fn__fails", () =>
      fake.putEnvelope(1, {
        code: "callTestWindowTooLarge",
        message: "window too large",
        data: { max: 5 },
      }),
    );

    let caught: unknown;
    try {
      callFn(fake.mod, "rspyts_fn__fails", {});
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(WindowTooLargeError);
    expect((caught as RspytsError).message).toBe("window too large");
    expect((caught as RspytsError).data).toEqual({ max: 5 });
    expect(fake.live.size).toBe(0);
  });

  it("throws RspytsPanicError on status 2", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__panics", () =>
      fake.putEnvelope(2, { code: "panic", message: "kaboom 7" }),
    );

    let caught: unknown;
    try {
      callFn(fake.mod, "rspyts_fn__panics", {});
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(RspytsPanicError);
    expect((caught as RspytsPanicError).message).toBe("kaboom 7");
  });

  it("rejects an export that returns a non-pointer, still freeing the request", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__bad", () => "not a pointer");

    expect(() =>
      callFn(fake.mod, "rspyts_fn__bad", { a: 1 }, [{ data: new Uint8Array([1]), dt: "u8" }]),
    ).toThrow(/returned a non-pointer/);
    fake.assertAllFreedOnce();
  });

  it("copies the envelope out BEFORE freeing it (zero-on-free fake)", () => {
    // The fake zeroes every freed range inside rspyts_free itself. If
    // callFn read any envelope byte after the free — or held a live view
    // instead of a copy — the payload would decode as zeros or garbage.
    const fake = createFake({ zeroOnFree: true });
    const data = new Int32Array([21, -22, 23]);
    fake.setExport("rspyts_fn__spectrum", () =>
      fake.putEnvelope(
        0,
        { bins: { __rspyts_buf__: { off: 0, len: 3, dt: "i32" } }, peak: 23 },
        new Uint8Array(data.buffer),
      ),
    );

    const result = callFn(fake.mod, "rspyts_fn__spectrum", {}) as {
      bins: Int32Array;
      peak: number;
    };

    expect(Array.from(result.bins)).toEqual([21, -22, 23]);
    expect(result.peak).toBe(23);
    fake.assertAllFreedOnce();
  });

  it("round-trips a 1 MiB envelope and survives later memory mutation", () => {
    const fake = createFake();
    const elements = 131072; // 1 MiB of f64 tail
    const data = new Float64Array(elements);
    for (let i = 0; i < elements; i++) {
      data[i] = i * 0.25;
    }
    fake.setExport("rspyts_fn__bulk", () =>
      fake.putEnvelope(
        0,
        { big: { __rspyts_buf__: { off: 0, len: elements, dt: "f64" } } },
        new Uint8Array(data.buffer),
      ),
    );

    const result = callFn(fake.mod, "rspyts_fn__bulk", {}) as { big: Float64Array };

    expect(result.big.length).toBe(elements);
    new Uint8Array(fake.memory.buffer).fill(0xff);
    expect(result.big[0]).toBe(0);
    expect(result.big[65536]).toBe(65536 * 0.25);
    expect(result.big[elements - 1]).toBe((elements - 1) * 0.25);
    fake.assertAllFreedOnce();
  });
});

describe("callFn memory discipline", () => {
  it("frees the request, slice, and envelope allocations with exact lengths", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__work", () => fake.putEnvelope(0, { ok: true }));

    callFn(fake.mod, "rspyts_fn__work", { a: 1 }, [{ data: new Float32Array([1]), dt: "f32" }]);

    // The fake throws on any length-mismatched or double free; all that is
    // left to check is that nothing leaked.
    expect(fake.live.size).toBe(0);
    expect(fake.freed).toHaveLength(3); // envelope, args, slice
  });

  it("frees every allocation exactly once on success with multiple slices", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__multi", () => fake.putEnvelope(0, null));

    callFn(fake.mod, "rspyts_fn__multi", { a: 1 }, [
      { data: new Uint8Array([1]), dt: "u8" },
      { data: new Int16Array([2]), dt: "i16" },
      { data: new Float64Array([3]), dt: "f64" },
    ]);

    fake.assertAllFreedOnce();
    expect(fake.freed).toHaveLength(5); // envelope, args, three slices
  });

  it("frees every allocation exactly once when the call returns an error envelope", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__fails", () =>
      fake.putEnvelope(1, { code: "someError", message: "nope" }),
    );

    expect(() =>
      callFn(fake.mod, "rspyts_fn__fails", { a: 1 }, [{ data: new Uint8Array([9]), dt: "u8" }]),
    ).toThrow(RspytsError);
    fake.assertAllFreedOnce();
    expect(fake.freed).toHaveLength(3); // envelope, args, slice
  });

  it("frees request buffers even when the export traps", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__traps", () => {
      throw new Error("unreachable executed");
    });

    expect(() =>
      callFn(fake.mod, "rspyts_fn__traps", { a: 1 }, [{ data: new Uint8Array([1]), dt: "u8" }]),
    ).toThrow("unreachable executed");
    expect(fake.live.size).toBe(0);
  });

  it("frees each request buffer exactly once when the export traps mid-call", () => {
    const fake = createFake();
    fake.setExport("rspyts_fn__traps", () => {
      throw new Error("unreachable executed");
    });

    expect(() =>
      callFn(fake.mod, "rspyts_fn__traps", { a: 1 }, [
        { data: new Float32Array([1.5]), dt: "f32" },
        { data: new Int32Array([-2]), dt: "i32" },
      ]),
    ).toThrow("unreachable executed");
    fake.assertAllFreedOnce();
    expect(fake.freed).toHaveLength(3); // args and both slices; no envelope existed
  });

  it("survives memory growth during allocation and mid-call", () => {
    // Every allocation grows memory by a page, detaching all prior views;
    // the export grows once more before writing the envelope. Correct
    // results prove that writeBytes/writeSlice re-view after every alloc
    // and that callFn re-views after the call.
    const fake = createFake({ growOnAlloc: true });
    const data = new Float64Array([4.25, -8.5]);
    let seenArgs: unknown;
    let seenSlice: number[] | undefined;
    fake.setExport(
      "rspyts_fn__grow",
      (argsPtr: number, argsLen: number, slicePtr: number, sliceLen: number) => {
        seenArgs = JSON.parse(utf8.decode(fake.readBytes(argsPtr, argsLen)));
        seenSlice = Array.from(new Float64Array(fake.memory.buffer, slicePtr, sliceLen));
        fake.memory.grow(1);
        return fake.putEnvelope(0, { echoed: true });
      },
    );

    const result = callFn(fake.mod, "rspyts_fn__grow", { n: 42 }, [{ data, dt: "f64" }]);

    expect(seenArgs).toEqual({ n: 42 });
    expect(seenSlice).toEqual([4.25, -8.5]);
    expect(result).toEqual({ echoed: true });
    expect(fake.live.size).toBe(0);
  });

  it("re-reads memory after the export grows it post-envelope (stale-view regression)", () => {
    const fake = createFake();
    const data = new Float64Array([3.5, -7.25]);
    fake.setExport("rspyts_fn__growLate", () => {
      const ptr = fake.putEnvelope(
        0,
        { bins: { __rspyts_buf__: { off: 0, len: 2, dt: "f64" } } },
        new Uint8Array(data.buffer),
      );
      // Grow AFTER the envelope is written and BEFORE returning: any view
      // callFn created before or during the call is now detached, so it
      // must build fresh views to read the header and copy the envelope.
      fake.memory.grow(1);
      return ptr;
    });

    const result = callFn(fake.mod, "rspyts_fn__growLate", {}) as { bins: Float64Array };

    expect(result.bins).toBeInstanceOf(Float64Array);
    expect(Array.from(result.bins)).toEqual([3.5, -7.25]);
    fake.assertAllFreedOnce();
  });

  it("survives growth on every allocation with all five slice dtypes at once", () => {
    // Worst case: every rspyts_alloc grows memory (detaching all previous
    // views), the export grows again mid-call, and once more after writing
    // the envelope. Args and all five slices must still arrive intact.
    const fake = createFake({ growOnAlloc: true });
    const seen: Record<string, number[]> = {};
    fake.setExport("rspyts_fn__growAll", (...raw: number[]) => {
      const [argsPtr, argsLen] = raw as [number, number];
      expect(utf8.decode(fake.readBytes(argsPtr, argsLen))).toBe('{"n":1}');
      fake.memory.grow(1);
      for (const [i, { dt, bytes, view }] of SLICE_CASES.entries()) {
        const ptr = raw[2 + i * 2]!;
        const count = raw[3 + i * 2]!;
        expect(ptr % bytes, dt).toBe(0);
        seen[dt] = Array.from(view(fake.memory.buffer, ptr, count));
      }
      const envPtr = fake.putEnvelope(0, { done: true });
      fake.memory.grow(1);
      return envPtr;
    });

    const result = callFn(
      fake.mod,
      "rspyts_fn__growAll",
      { n: 1 },
      SLICE_CASES.map(({ dt, make, values }) => ({ data: make(values), dt })),
    );

    expect(result).toEqual({ done: true });
    for (const { dt, make, values } of SLICE_CASES) {
      expect(seen[dt], dt).toEqual(Array.from(make(values)));
    }
    fake.assertAllFreedOnce();
  });
});

describe("writeSlice", () => {
  it("over-allocates and returns an aligned interior pointer", () => {
    const fake = createFake();
    fake.alloc(1); // misalign the bump pointer
    const sa = writeSlice(fake.mod, { data: new Float64Array([42]), dt: "f64" });
    expect(sa.len).toBe(16); // 8 data bytes + 8 alignment slack
    expect(sa.count).toBe(1);
    expect(sa.aligned % 8).toBe(0);
    expect(sa.aligned).toBeGreaterThanOrEqual(sa.ptr);
    expect(sa.aligned + 8).toBeLessThanOrEqual(sa.ptr + sa.len);
    expect(Array.from(new Float64Array(fake.memory.buffer, sa.aligned, 1))).toEqual([42]);
  });

  it("over-allocates by exactly the element alignment for every dtype", () => {
    for (const { dt, bytes, make, view, values } of SLICE_CASES) {
      const fake = createFake();
      fake.misalign(3); // misalign the bump pointer
      const sa = writeSlice(fake.mod, { data: make(values), dt });
      expect(sa.len, dt).toBe(values.length * bytes + bytes);
      expect(sa.count, dt).toBe(values.length);
      expect(sa.aligned % bytes, dt).toBe(0);
      expect(sa.aligned, dt).toBeGreaterThanOrEqual(sa.ptr);
      expect(sa.aligned + values.length * bytes, dt).toBeLessThanOrEqual(sa.ptr + sa.len);
      const written = Array.from(view(fake.memory.buffer, sa.aligned, values.length));
      expect(written, dt).toEqual(Array.from(make(values)));
    }
  });

  it("copies from a typed array that views a larger buffer at an offset", () => {
    const fake = createFake();
    const backing = new Float64Array([0, 1.5, -2.5, 0]);
    const window = backing.subarray(1, 3);
    const sa = writeSlice(fake.mod, { data: window, dt: "f64" });
    expect(sa.count).toBe(2);
    expect(Array.from(new Float64Array(fake.memory.buffer, sa.aligned, 2))).toEqual([1.5, -2.5]);
  });

  it("rejects a typed array that does not match the declared dtype", () => {
    const fake = createFake();
    expect(() => writeSlice(fake.mod, { data: new Float32Array([1]), dt: "f64" })).toThrow(
      /requires a Float64Array/,
    );
  });
});

describe("writeBytes", () => {
  it("handles zero-length input without recording an allocation", () => {
    const fake = createFake();
    const allocation = writeBytes(fake.mod, new Uint8Array(0));
    expect(allocation.len).toBe(0);
    expect(fake.live.size).toBe(0);
  });
});

describe("callDrop", () => {
  it("invokes the export with the bigint handle", () => {
    const fake = createFake();
    let dropped: unknown;
    fake.setExport("rspyts_cls__Stats__drop", (handle: bigint) => {
      dropped = handle;
    });

    callDrop(fake.mod, "rspyts_cls__Stats__drop", 9n);

    expect(dropped).toBe(9n);
  });

  it("swallows a trapping drop (fire-and-forget)", () => {
    const fake = createFake();
    fake.setExport("rspyts_cls__Stats__drop", () => {
      throw new Error("trap");
    });
    expect(() => callDrop(fake.mod, "rspyts_cls__Stats__drop", 9n)).not.toThrow();
  });

  it("still surfaces a missing export", () => {
    const fake = createFake();
    expect(() => callDrop(fake.mod, "rspyts_cls__Gone__drop", 1n)).toThrow(/no export/);
  });
});
