import { describe, expect, it } from "vitest";

import {
  BUF_PLACEHOLDER_KEY,
  HEADER_LEN,
  STATUS_ERROR,
  STATUS_OK,
  decodeEnvelope,
  substituteBuffers,
  type BufTypedArray,
  type Dtype,
} from "../src/envelope.js";
import { buildEnvelope } from "./fake.js";

function placeholder(off: number, len: number, dt: string): Record<string, unknown> {
  return { [BUF_PLACEHOLDER_KEY]: { off, len, dt } };
}

interface DtypeCase {
  dt: Dtype;
  bytes: number;
  ctor: new (values: number[]) => BufTypedArray;
  values: number[];
}

/** One representative value set per wire dtype, exact in every type. */
const DTYPE_CASES: DtypeCase[] = [
  { dt: "u8", bytes: 1, ctor: Uint8Array, values: [0, 127, 255] },
  { dt: "i16", bytes: 2, ctor: Int16Array, values: [-32768, -1, 32767] },
  { dt: "i32", bytes: 4, ctor: Int32Array, values: [-2147483648, 0, 2147483647] },
  { dt: "f32", bytes: 4, ctor: Float32Array, values: [-0.5, 1.25, 1024] },
  { dt: "f64", bytes: 8, ctor: Float64Array, values: [Math.PI, -1e300, 0.1] },
];

describe("decodeEnvelope", () => {
  it("decodes an ok envelope", () => {
    const { status, payload } = decodeEnvelope(buildEnvelope(0, '{"a":1,"b":[true,null]}'));
    expect(status).toBe(STATUS_OK);
    expect(payload).toEqual({ a: 1, b: [true, null] });
  });

  it("parses the little-endian header and splits json from tail", () => {
    const tail = new Uint8Array([9, 8, 7]);
    const { status, payload } = decodeEnvelope(buildEnvelope(1, '"boom"', tail));
    expect(status).toBe(STATUS_ERROR);
    expect(payload).toBe("boom");
  });

  it("substitutes placeholders in ok payloads", () => {
    const data = new Float64Array([1.5, -2.5]);
    const { payload } = decodeEnvelope(
      buildEnvelope(0, JSON.stringify({ values: placeholder(0, 2, "f64") }), new Uint8Array(data.buffer)),
    );
    const values = (payload as { values: Float64Array }).values;
    expect(values).toBeInstanceOf(Float64Array);
    expect(Array.from(values)).toEqual([1.5, -2.5]);
  });

  it("leaves error payloads untouched, even placeholder-shaped ones", () => {
    const json = JSON.stringify({ code: "x", message: "y", data: placeholder(0, 1, "u8") });
    const { status, payload } = decodeEnvelope(buildEnvelope(1, json));
    expect(status).toBe(STATUS_ERROR);
    expect((payload as { data: unknown }).data).toEqual(placeholder(0, 1, "u8"));
  });

  it("rejects envelopes shorter than the header", () => {
    expect(() => decodeEnvelope(new Uint8Array(4))).toThrow(/truncated envelope/);
  });

  it("rejects envelopes shorter than the header promises", () => {
    const bytes = buildEnvelope(0, '{"a":1}');
    expect(() => decodeEnvelope(bytes.subarray(0, bytes.byteLength - 2))).toThrow(
      /truncated envelope/,
    );
  });

  it("round-trips every dtype, tail view deliberately misaligned in the envelope", () => {
    for (const { dt, bytes, ctor, values } of DTYPE_CASES) {
      const data = new ctor(values);
      // Pad the JSON so the tail starts at an ODD offset inside the
      // envelope — unaligned relative to the backing buffer for every
      // multi-byte dtype. The decoder must copy, never view in place.
      let payload: Record<string, unknown> = { buf: placeholder(0, values.length, dt), pad: "" };
      if ((HEADER_LEN + JSON.stringify(payload).length) % 2 === 0) {
        payload = { buf: placeholder(0, values.length, dt), pad: "x" };
      }
      const json = JSON.stringify(payload);
      expect((HEADER_LEN + json.length) % 2, dt).toBe(1);
      const envelope = buildEnvelope(0, json, new Uint8Array(data.buffer));
      const { status, payload: decoded } = decodeEnvelope(envelope);
      expect(status, dt).toBe(STATUS_OK);
      const buf = (decoded as { buf: BufTypedArray }).buf;
      expect(buf, dt).toBeInstanceOf(ctor);
      expect(Array.from(buf), dt).toEqual(Array.from(data));
    }
  });

  it("decodes a zero-length buffer from a zero-length tail", () => {
    const { status, payload } = decodeEnvelope(
      buildEnvelope(0, JSON.stringify({ empty: placeholder(0, 0, "f64") })),
    );
    expect(status).toBe(STATUS_OK);
    const empty = (payload as { empty: Float64Array }).empty;
    expect(empty).toBeInstanceOf(Float64Array);
    expect(empty.length).toBe(0);
  });

  it("walks into extra-key objects without substituting them", () => {
    const tail = new Uint8Array([5]);
    const json = JSON.stringify({
      outer: {
        [BUF_PLACEHOLDER_KEY]: { off: 0, len: 1, dt: "u8" },
        inner: placeholder(0, 1, "u8"),
      },
    });
    const { payload } = decodeEnvelope(buildEnvelope(0, json, tail));
    const outer = (payload as { outer: Record<string, unknown> }).outer;
    // The two-key object is NOT a placeholder, but the walk still recurses
    // into it and substitutes the genuine placeholder underneath.
    expect(outer[BUF_PLACEHOLDER_KEY]).toEqual({ off: 0, len: 1, dt: "u8" });
    expect(outer["inner"]).toBeInstanceOf(Uint8Array);
    expect(Array.from(outer["inner"] as Uint8Array)).toEqual([5]);
  });

  it("round-trips a 1 MiB payload", () => {
    const elements = 131072; // 131072 f64 elements = exactly 1 MiB of tail
    const data = new Float64Array(elements);
    for (let i = 0; i < elements; i++) {
      data[i] = i * 0.5;
    }
    const json = JSON.stringify({
      big: placeholder(0, elements, "f64"),
      note: "y".repeat(4096),
    });
    const { status, payload } = decodeEnvelope(
      buildEnvelope(0, json, new Uint8Array(data.buffer)),
    );
    expect(status).toBe(STATUS_OK);
    const big = (payload as { big: Float64Array }).big;
    expect(big).toBeInstanceOf(Float64Array);
    expect(big.length).toBe(elements);
    expect(big[0]).toBe(0);
    expect(big[65536]).toBe(65536 * 0.5);
    expect(big[elements - 1]).toBe((elements - 1) * 0.5);
  });
});

describe("substituteBuffers", () => {
  it("materializes every dtype", () => {
    const cases: Array<{ dt: string; data: ArrayLike<number> & { buffer: ArrayBufferLike }; expected: number[]; ctor: Function }> = [
      { dt: "u8", data: new Uint8Array([1, 2, 255]), expected: [1, 2, 255], ctor: Uint8Array },
      { dt: "i16", data: new Int16Array([-3, 1000]), expected: [-3, 1000], ctor: Int16Array },
      { dt: "i32", data: new Int32Array([-70000, 5]), expected: [-70000, 5], ctor: Int32Array },
      { dt: "f32", data: new Float32Array([1.5, -2.25]), expected: [1.5, -2.25], ctor: Float32Array },
      { dt: "f64", data: new Float64Array([Math.PI]), expected: [Math.PI], ctor: Float64Array },
    ];
    for (const { dt, data, expected, ctor } of cases) {
      const tail = new Uint8Array(data.buffer.slice(0));
      const result = substituteBuffers(placeholder(0, data.length, dt), tail);
      expect(result, dt).toBeInstanceOf(ctor);
      expect(Array.from(result as ArrayLike<number>), dt).toEqual(expected);
    }
  });

  it("returns a copy independent of the tail", () => {
    const tail = new Uint8Array(new Int32Array([11, 22]).buffer);
    const result = substituteBuffers(placeholder(0, 2, "i32"), tail) as Int32Array;
    tail.fill(0);
    expect(Array.from(result)).toEqual([11, 22]);
  });

  it("handles a tail view that is unaligned relative to its buffer", () => {
    // The tail follows the JSON bytes inside the envelope, so its
    // byteOffset is arbitrary. A direct typed-array view over the backing
    // buffer would throw on alignment; the copying strategy must not.
    const nums = new Float64Array([7.5, -0.125]);
    const backing = new Uint8Array(1 + nums.byteLength);
    backing.set(new Uint8Array(nums.buffer), 1);
    const tail = backing.subarray(1);
    expect(tail.byteOffset % 8).not.toBe(0);
    const result = substituteBuffers(placeholder(0, 2, "f64"), tail) as Float64Array;
    expect(Array.from(result)).toEqual([7.5, -0.125]);
  });

  it("honors non-zero offsets (aligned tail padding)", () => {
    // Layout mirrors rspyts-core tail_push: 1 byte, 7 bytes zero padding,
    // then one f64 at offset 8.
    const tail = new Uint8Array(16);
    tail[0] = 42;
    tail.set(new Uint8Array(new Float64Array([2.5]).buffer), 8);
    const result = substituteBuffers(placeholder(8, 1, "f64"), tail) as Float64Array;
    expect(Array.from(result)).toEqual([2.5]);
  });

  it("recurses through arrays and objects", () => {
    const tail = new Uint8Array([10, 20]);
    const value = {
      a: [placeholder(0, 1, "u8"), 2, "x"],
      b: { c: placeholder(1, 1, "u8"), d: null },
      e: true,
    };
    const result = substituteBuffers(value, tail) as {
      a: [Uint8Array, number, string];
      b: { c: Uint8Array; d: null };
      e: boolean;
    };
    expect(Array.from(result.a[0])).toEqual([10]);
    expect(result.a[1]).toBe(2);
    expect(Array.from(result.b.c)).toEqual([20]);
    expect(result.b.d).toBeNull();
    expect(result.e).toBe(true);
  });

  it("substitutes a bare top-level placeholder", () => {
    const tail = new Uint8Array(new Int16Array([5, -6]).buffer);
    const result = substituteBuffers(placeholder(0, 2, "i16"), tail) as Int16Array;
    expect(Array.from(result)).toEqual([5, -6]);
  });

  it("ignores objects where the placeholder key is not the sole key", () => {
    const tail = new Uint8Array([1]);
    const value = { [BUF_PLACEHOLDER_KEY]: { off: 0, len: 1, dt: "u8" }, extra: 1 };
    expect(substituteBuffers(value, tail)).toEqual(value);
  });

  it("copies from an unaligned tail offset for every dtype", () => {
    for (const { dt, bytes, ctor, values } of DTYPE_CASES) {
      const data = new ctor(values);
      // One junk byte before the data puts every multi-byte dtype at an
      // offset that is NOT a multiple of its element size.
      const tail = new Uint8Array(1 + data.byteLength);
      tail[0] = 0xaa;
      tail.set(new Uint8Array(data.buffer), 1);
      if (bytes > 1) {
        expect(1 % bytes, dt).not.toBe(0);
      }
      const result = substituteBuffers(placeholder(1, values.length, dt), tail);
      expect(result, dt).toBeInstanceOf(ctor);
      expect(Array.from(result as BufTypedArray), dt).toEqual(Array.from(data));
    }
  });

  it("substitutes placeholders nested three levels deep", () => {
    const tail = new Uint8Array(new Int32Array([-5, 6, 7]).buffer);
    const value = {
      level1: {
        level2: {
          level3: placeholder(0, 2, "i32"),
        },
        siblings: [[placeholder(8, 1, "i32")]],
      },
    };
    const result = substituteBuffers(value, tail) as {
      level1: {
        level2: { level3: Int32Array };
        siblings: [[Int32Array]];
      };
    };
    expect(result.level1.level2.level3).toBeInstanceOf(Int32Array);
    expect(Array.from(result.level1.level2.level3)).toEqual([-5, 6]);
    expect(result.level1.siblings[0][0]).toBeInstanceOf(Int32Array);
    expect(Array.from(result.level1.siblings[0][0])).toEqual([7]);
  });

  it("materializes zero-length buffers for every dtype from an empty tail", () => {
    for (const { dt, ctor } of DTYPE_CASES) {
      const result = substituteBuffers(placeholder(0, 0, dt), new Uint8Array(0));
      expect(result, dt).toBeInstanceOf(ctor);
      expect((result as BufTypedArray).length, dt).toBe(0);
    }
  });

  it("rejects out-of-range placeholders", () => {
    const tail = new Uint8Array(8);
    expect(() => substituteBuffers(placeholder(8, 1, "f64"), tail)).toThrow(/exceeds tail/);
  });

  it("rejects negative offsets", () => {
    expect(() => substituteBuffers(placeholder(-8, 1, "u8"), new Uint8Array(8))).toThrow(
      /exceeds tail/,
    );
  });

  it("rejects unknown dtypes", () => {
    expect(() => substituteBuffers(placeholder(0, 1, "f16"), new Uint8Array(8))).toThrow(
      /malformed buffer placeholder/,
    );
  });

  it("rejects placeholder bodies that are not objects", () => {
    for (const body of [null, 3, "off=0"]) {
      expect(() =>
        substituteBuffers({ [BUF_PLACEHOLDER_KEY]: body }, new Uint8Array(8)),
      ).toThrow(/body is not an object/);
    }
  });

  it("rejects placeholder bodies with missing or mistyped fields", () => {
    const bodies = [
      { len: 1, dt: "u8" }, // no off
      { off: "0", len: 1, dt: "u8" }, // off not a number
      { off: 0, dt: "u8" }, // no len
      { off: 0, len: 1 }, // no dt
      { off: 0, len: 1, dt: 8 }, // dt not a string
    ];
    for (const body of bodies) {
      expect(() =>
        substituteBuffers({ [BUF_PLACEHOLDER_KEY]: body }, new Uint8Array(8)),
      ).toThrow(/malformed buffer placeholder/);
    }
  });
});
