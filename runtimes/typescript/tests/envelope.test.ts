import { describe, expect, it } from "vitest";

import {
  BUF_PLACEHOLDER_KEY,
  HEADER_LEN,
  JSON_WRAPPER_KEY,
  STATUS_ERROR,
  STATUS_OK,
  buildRequest,
  decodeEnvelope,
  decodeRequest,
  substituteBuffers,
  wireBuffer,
  type BufTypedArray,
  type Dtype,
} from "../src/envelope.js";
import { buildEnvelope } from "./fake.js";

function placeholder(off: number, len: number, dt: string): Record<string, unknown> {
  return { [BUF_PLACEHOLDER_KEY]: { off, len, dt } };
}

function valuesOf(array: BufTypedArray): Array<number | bigint> {
  return Array.from(array as unknown as ArrayLike<number | bigint>);
}

interface DtypeCase {
  dt: Dtype;
  bytes: number;
  ctor: Function;
  make: () => BufTypedArray;
}

/** One representative value set per wire dtype, exact in every type. */
const DTYPE_CASES: DtypeCase[] = [
  { dt: "u8", bytes: 1, ctor: Uint8Array, make: () => new Uint8Array([0, 127, 255]) },
  { dt: "i8", bytes: 1, ctor: Int8Array, make: () => new Int8Array([-128, -1, 127]) },
  { dt: "u16", bytes: 2, ctor: Uint16Array, make: () => new Uint16Array([0, 32768, 65535]) },
  { dt: "i16", bytes: 2, ctor: Int16Array, make: () => new Int16Array([-32768, -1, 32767]) },
  { dt: "u32", bytes: 4, ctor: Uint32Array, make: () => new Uint32Array([0, 2147483648, 4294967295]) },
  { dt: "i32", bytes: 4, ctor: Int32Array, make: () => new Int32Array([-2147483648, 0, 2147483647]) },
  { dt: "u64", bytes: 8, ctor: BigUint64Array, make: () => new BigUint64Array([0n, 1n << 63n, (1n << 64n) - 1n]) },
  { dt: "i64", bytes: 8, ctor: BigInt64Array, make: () => new BigInt64Array([-(1n << 63n), -1n, (1n << 63n) - 1n]) },
  { dt: "f32", bytes: 4, ctor: Float32Array, make: () => new Float32Array([-0.5, 1.25, 1024]) },
  { dt: "f64", bytes: 8, ctor: Float64Array, make: () => new Float64Array([Math.PI, -1e300, 0.1]) },
];

describe("buildRequest", () => {
  it("encodes every dtype as a nested aligned little-endian attachment", () => {
    const args = {
      nested: DTYPE_CASES.map(({ dt, make }) => ({ [dt]: wireBuffer(make(), dt) })),
    };
    const request = buildRequest(args);
    const decoded = decodeRequest(request);
    const payload = decoded.payload as { nested: Array<Record<string, unknown>> };

    for (const [index, { dt, bytes, make }] of DTYPE_CASES.entries()) {
      const spec = (payload.nested[index]![dt] as Record<string, Record<string, unknown>>)[
        BUF_PLACEHOLDER_KEY
      ]!;
      expect(spec["dt"], dt).toBe(dt);
      expect(spec["len"], dt).toBe(make().length);
      expect((spec["off"] as number) % bytes, dt).toBe(0);
    }

    const materialized = substituteBuffers(decoded.payload, decoded.tail) as {
      nested: Array<Record<string, BufTypedArray>>;
    };
    for (const [index, { dt, make }] of DTYPE_CASES.entries()) {
      expect(valuesOf(materialized.nested[index]![dt]!), dt).toEqual(valuesOf(make()));
    }
  });

  it("frames plain JSON with an empty tail", () => {
    const request = buildRequest({ a: 1, items: [true, null, "x"] });
    const decoded = decodeRequest(request);
    expect(decoded.payload).toEqual({ a: 1, items: [true, null, "x"] });
    expect(decoded.tail.byteLength).toBe(0);
    expect(Array.from(request.subarray(0, 4))).toEqual([0, 0, 0, 0]);
  });

  it("distinguishes bare bytes from explicitly marked numeric u8 buffers", () => {
    const request = decodeRequest(
      buildRequest({ bytes: new Uint8Array([0, 255]), numeric: wireBuffer(new Uint8Array([1]), "u8") }),
    );
    const payload = request.payload as Record<string, Record<string, Record<string, unknown>>>;
    expect(payload["bytes"]![BUF_PLACEHOLDER_KEY]!["dt"]).toBe("bytes");
    expect(payload["numeric"]![BUF_PLACEHOLDER_KEY]!["dt"]).toBe("u8");
    const materialized = substituteBuffers(request.payload, request.tail) as {
      bytes: Uint8Array;
      numeric: Uint8Array;
    };
    expect(Array.from(materialized.bytes)).toEqual([0, 255]);
    expect(Array.from(materialized.numeric)).toEqual([1]);

    expect(() => buildRequest({ values: new Int8Array([1]) })).toThrow(/wireBuffer/);
    expect(() => wireBuffer(new Uint8Array([1]), "i8")).toThrow(/requires a Int8Array/);
  });

  it("strictly decodes request direction, lengths, and reserved bytes", () => {
    const request = buildRequest({ value: 1 });

    const marker = request.slice();
    marker[0] = 1;
    expect(() => decodeRequest(marker)).toThrow(/invalid request marker/);

    const reserved = request.slice();
    reserved[2] = 1;
    expect(() => decodeRequest(reserved)).toThrow(/reserved envelope/);

    expect(() => decodeRequest(request.subarray(0, request.byteLength - 1))).toThrow(/truncated/);
    const trailing = new Uint8Array(request.byteLength + 1);
    trailing.set(request);
    expect(() => decodeRequest(trailing)).toThrow(/trailing bytes/);
  });
});

describe("decodeEnvelope", () => {
  it("decodes an ok envelope", () => {
    const { status, payload } = decodeEnvelope(buildEnvelope(0, '{"a":1,"b":[true,null]}'));
    expect(status).toBe(STATUS_OK);
    expect(payload).toEqual({ a: 1, b: [true, null] });
  });

  it("parses the little-endian header and splits json from tail", () => {
    const tail = new Uint8Array([9, 8, 7]);
    const { status, payload } = decodeEnvelope(buildEnvelope(0, '"done"', tail));
    expect(status).toBe(STATUS_OK);
    expect(payload).toBe("done");
  });

  it("rejects attachment tails on error and panic envelopes", () => {
    expect(() => decodeEnvelope(buildEnvelope(1, '"boom"', new Uint8Array([7])))).toThrow(
      /must not contain attachment bytes/,
    );
    expect(() => decodeEnvelope(buildEnvelope(2, '"boom"', new Uint8Array([7])))).toThrow(
      /must not contain attachment bytes/,
    );
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

  it("keeps marker-shaped content inside Json opaque and unwraps it losslessly", () => {
    const marker = placeholder(0, 1, "u8");
    const json = JSON.stringify({ [JSON_WRAPPER_KEY]: { marker, nested: [{ marker }] } });
    const { payload } = decodeEnvelope(buildEnvelope(0, json));
    expect(payload).toEqual({ marker, nested: [{ marker }] });

    const request = decodeRequest(
      buildRequest({ value: { [JSON_WRAPPER_KEY]: { marker, nested: [{ marker }] } } }),
    );
    expect(request.tail.byteLength).toBe(0);
    expect(request.payload).toEqual({
      value: { [JSON_WRAPPER_KEY]: { marker, nested: [{ marker }] } },
    });
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

  it("rejects unknown statuses, reserved bytes, trailing bytes, and malformed JSON", () => {
    expect(() => decodeEnvelope(buildEnvelope(3, "null"))).toThrow(/invalid response status/);

    const reserved = buildEnvelope(0, "null");
    reserved[1] = 1;
    expect(() => decodeEnvelope(reserved)).toThrow(/reserved envelope/);

    const valid = buildEnvelope(0, "null");
    const trailing = new Uint8Array(valid.byteLength + 1);
    trailing.set(valid);
    expect(() => decodeEnvelope(trailing)).toThrow(/trailing bytes/);

    expect(() => decodeEnvelope(buildEnvelope(0, "{"))).toThrow(/malformed envelope JSON/);
  });

  it("round-trips every dtype, tail view deliberately misaligned in the envelope", () => {
    for (const { dt, bytes, ctor, make } of DTYPE_CASES) {
      const data = make();
      // Pad the JSON so the tail starts at an ODD offset inside the
      // envelope — unaligned relative to the backing buffer for every
      // multi-byte dtype. The decoder must copy, never view in place.
      let payload: Record<string, unknown> = { buf: placeholder(0, data.length, dt), pad: "" };
      if ((HEADER_LEN + JSON.stringify(payload).length) % 2 === 0) {
        payload = { buf: placeholder(0, data.length, dt), pad: "x" };
      }
      const json = JSON.stringify(payload);
      expect((HEADER_LEN + json.length) % 2, dt).toBe(1);
      const envelope = buildEnvelope(0, json, new Uint8Array(data.buffer));
      const { status, payload: decoded } = decodeEnvelope(envelope);
      expect(status, dt).toBe(STATUS_OK);
      const buf = (decoded as { buf: BufTypedArray }).buf;
      expect(buf, dt).toBeInstanceOf(ctor);
      expect(valuesOf(buf), dt).toEqual(valuesOf(data));
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

  it("rejects placeholder keys with sibling fields", () => {
    const tail = new Uint8Array([5]);
    const json = JSON.stringify({
      outer: {
        [BUF_PLACEHOLDER_KEY]: { off: 0, len: 1, dt: "u8" },
        inner: placeholder(0, 1, "u8"),
      },
    });
    expect(() => decodeEnvelope(buildEnvelope(0, json, tail))).toThrow(/sibling fields/);
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
    const cases: Array<{
      dt: string;
      data: BufTypedArray;
      expected: Array<number | bigint>;
      ctor: Function;
    }> = [
      { dt: "u8", data: new Uint8Array([1, 2, 255]), expected: [1, 2, 255], ctor: Uint8Array },
      { dt: "i8", data: new Int8Array([-128, 0, 127]), expected: [-128, 0, 127], ctor: Int8Array },
      { dt: "u16", data: new Uint16Array([0, 65535]), expected: [0, 65535], ctor: Uint16Array },
      { dt: "i16", data: new Int16Array([-3, 1000]), expected: [-3, 1000], ctor: Int16Array },
      { dt: "u32", data: new Uint32Array([0, 4294967295]), expected: [0, 4294967295], ctor: Uint32Array },
      { dt: "i32", data: new Int32Array([-70000, 5]), expected: [-70000, 5], ctor: Int32Array },
      { dt: "u64", data: new BigUint64Array([0n, (1n << 64n) - 1n]), expected: [0n, (1n << 64n) - 1n], ctor: BigUint64Array },
      { dt: "i64", data: new BigInt64Array([-(1n << 63n), (1n << 63n) - 1n]), expected: [-(1n << 63n), (1n << 63n) - 1n], ctor: BigInt64Array },
      { dt: "f32", data: new Float32Array([1.5, -2.25]), expected: [1.5, -2.25], ctor: Float32Array },
      { dt: "f64", data: new Float64Array([Math.PI]), expected: [Math.PI], ctor: Float64Array },
    ];
    for (const { dt, data, expected, ctor } of cases) {
      const tail = new Uint8Array(data.buffer.slice(0));
      const result = substituteBuffers(placeholder(0, data.length, dt), tail);
      expect(result, dt).toBeInstanceOf(ctor);
      expect(valuesOf(result as BufTypedArray), dt).toEqual(expected);
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

  it("rejects objects where the placeholder key is not the sole key", () => {
    const tail = new Uint8Array([1]);
    const value = { [BUF_PLACEHOLDER_KEY]: { off: 0, len: 1, dt: "u8" }, extra: 1 };
    expect(() => substituteBuffers(value, tail)).toThrow(/sibling fields/);
  });

  it("rejects unaligned placeholder offsets for multi-byte dtypes", () => {
    for (const { dt, bytes, ctor, make } of DTYPE_CASES) {
      const data = make();
      // One junk byte before the data puts every multi-byte dtype at an
      // offset that is NOT a multiple of its element size.
      const tail = new Uint8Array(1 + data.byteLength);
      tail[0] = 0xaa;
      tail.set(new Uint8Array(data.buffer), 1);
      if (bytes > 1) {
        expect(1 % bytes, dt).not.toBe(0);
      }
      if (bytes === 1) {
        const result = substituteBuffers(placeholder(1, data.length, dt), tail);
        expect(result, dt).toBeInstanceOf(ctor);
        expect(valuesOf(result as BufTypedArray), dt).toEqual(valuesOf(data));
      } else {
        expect(() => substituteBuffers(placeholder(1, data.length, dt), tail), dt).toThrow(
          /not aligned/,
        );
      }
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
      { off: 0, len: 1, dt: "u8", extra: true }, // extra field
    ];
    for (const body of bodies) {
      expect(() =>
        substituteBuffers({ [BUF_PLACEHOLDER_KEY]: body }, new Uint8Array(8)),
      ).toThrow(/malformed buffer placeholder/);
    }
  });
});
