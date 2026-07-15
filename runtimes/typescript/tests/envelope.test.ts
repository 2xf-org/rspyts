import { describe, expect, it } from "vitest";

import {
  BUF_PLACEHOLDER_KEY,
  HEADER_LEN,
  STATUS_ERROR,
  STATUS_OK,
  bufferFromEnvelope,
  buildRequest,
  decodeEnvelope,
  decodeRequest,
  jsonToWire,
  wireBuffer,
  type BufTypedArray,
  type Dtype,
} from "../src/envelope.js";
import { buildEnvelope } from "./fake.js";

const placeholder = (off: number, len: number, dt: Dtype | "bytes") => ({
  [BUF_PLACEHOLDER_KEY]: { off, len, dt },
});

const valuesOf = (value: BufTypedArray): Array<number | bigint> =>
  Array.from(value as unknown as ArrayLike<number | bigint>);

const DTYPE_CASES: Array<{
  dt: Dtype;
  bytes: number;
  ctor: Function;
  make: () => BufTypedArray;
}> = [
  { dt: "u8", bytes: 1, ctor: Uint8Array, make: () => new Uint8Array([0, 127, 255]) },
  { dt: "i8", bytes: 1, ctor: Int8Array, make: () => new Int8Array([-128, -1, 127]) },
  { dt: "u16", bytes: 2, ctor: Uint16Array, make: () => new Uint16Array([0, 32768, 65535]) },
  { dt: "i16", bytes: 2, ctor: Int16Array, make: () => new Int16Array([-32768, -1, 32767]) },
  { dt: "u32", bytes: 4, ctor: Uint32Array, make: () => new Uint32Array([0, 2147483648, 4294967295]) },
  { dt: "i32", bytes: 4, ctor: Int32Array, make: () => new Int32Array([-2147483648, 0, 2147483647]) },
  { dt: "u64", bytes: 8, ctor: BigUint64Array, make: () => new BigUint64Array([0n, 1n << 63n]) },
  { dt: "i64", bytes: 8, ctor: BigInt64Array, make: () => new BigInt64Array([-(1n << 63n), -1n]) },
  { dt: "f32", bytes: 4, ctor: Float32Array, make: () => new Float32Array([-0.5, 1.25]) },
  { dt: "f64", bytes: 8, ctor: Float64Array, make: () => new Float64Array([Math.PI, -1e300]) },
];

describe("ABI-3 request encoding", () => {
  it("encodes every numeric dtype as an aligned little-endian attachment", () => {
    const request = decodeRequest(
      buildRequest({ values: DTYPE_CASES.map(({ dt, make }) => wireBuffer(make(), dt)) }),
    );
    const values = (request.value as { values: unknown[] }).values;
    for (const [index, { dt, bytes, make }] of DTYPE_CASES.entries()) {
      const wrapper = values[index] as Record<string, Record<string, unknown>>;
      const spec = wrapper[BUF_PLACEHOLDER_KEY]!;
      expect(spec["dt"], dt).toBe(dt);
      expect(spec["len"], dt).toBe(make().length);
      expect((spec["off"] as number) % bytes, dt).toBe(0);
      expect(
        valuesOf(bufferFromEnvelope(wrapper, request.tail, dt)),
        dt,
      ).toEqual(valuesOf(make()));
    }
  });

  it("keeps Json transparent and collision-proof against transport markers", () => {
    const reserved = {
      [BUF_PLACEHOLDER_KEY]: { off: 999, len: 4, dt: "f64" },
      __rspytsWireBuffer: true,
      data: [],
      dt: "u8",
    };
    const request = decodeRequest(buildRequest({ value: jsonToWire(reserved) }));
    expect(request.value).toEqual({ value: reserved });
    expect(request.tail.byteLength).toBe(0);
  });

  it("distinguishes Bytes from Buf<u8> and copies input bytes", () => {
    const bytes = new Uint8Array([1, 2]);
    const request = decodeRequest(
      buildRequest({ bytes, numeric: wireBuffer(new Uint8Array([3, 4]), "u8") }),
    );
    bytes.fill(9);
    const value = request.value as Record<string, unknown>;
    expect(valuesOf(bufferFromEnvelope(value["bytes"], request.tail, "bytes"))).toEqual([1, 2]);
    expect(valuesOf(bufferFromEnvelope(value["numeric"], request.tail, "u8"))).toEqual([3, 4]);
  });

  it("rejects invalid JSON and unmarked numeric typed arrays", () => {
    expect(() => buildRequest({ value: Number.NaN })).toThrow(/non-finite/);
    expect(() => jsonToWire({ value: new Uint8Array([1]) })).toThrow(/plain objects/);
    expect(() => jsonToWire({ value: Number.MIN_SAFE_INTEGER - 1 })).toThrow(/safe integers/);
    expect(() => buildRequest({ value: new Int16Array([1]) })).toThrow(/wireBuffer/);
    expect(() => wireBuffer(new Uint8Array([1]), "i8")).toThrow(/Int8Array/);
    const cyclic: Record<string, unknown> = {};
    cyclic["self"] = cyclic;
    expect(() => jsonToWire(cyclic)).toThrow(/cycles/);
    const sparse = new Array(2);
    sparse[1] = "present";
    expect(() => jsonToWire(sparse)).toThrow(/sparse holes/);
    expect(() => jsonToWire({ [Symbol("hidden")]: true })).toThrow(/symbol properties/);
    const hidden = {};
    Object.defineProperty(hidden, "secret", { value: 1, enumerable: false });
    expect(() => jsonToWire(hidden)).toThrow(/enumerable data properties/);
  });

  it("strictly validates request framing", () => {
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

describe("ABI-3 response decoding", () => {
  it("returns explicit value and tail without scanning reserved-looking maps", () => {
    const reserved = placeholder(0, 1, "u8");
    const decoded = decodeEnvelope(
      buildEnvelope(STATUS_OK, JSON.stringify({ reserved }), new Uint8Array([7])),
    );
    expect(decoded.value).toEqual({ reserved });
    expect(Array.from(decoded.tail)).toEqual([7]);
  });

  it("rejects invalid status, reserved bytes, truncation, trailing bytes, and malformed JSON", () => {
    expect(() => decodeEnvelope(new Uint8Array(4))).toThrow(/truncated envelope/);
    expect(() => decodeEnvelope(buildEnvelope(3, "null"))).toThrow(/invalid response status/);
    const reserved = buildEnvelope(0, "null");
    reserved[1] = 1;
    expect(() => decodeEnvelope(reserved)).toThrow(/reserved envelope/);
    const valid = buildEnvelope(0, "null");
    expect(() => decodeEnvelope(valid.subarray(0, valid.length - 1))).toThrow(/truncated/);
    const trailing = new Uint8Array(valid.length + 1);
    trailing.set(valid);
    expect(() => decodeEnvelope(trailing)).toThrow(/trailing bytes/);
    expect(() => decodeEnvelope(buildEnvelope(0, "{"))).toThrow(/malformed envelope JSON/);
  });

  it("rejects tails on application-error and panic envelopes", () => {
    expect(() => decodeEnvelope(buildEnvelope(1, "{}", new Uint8Array([1])))).toThrow(
      /must not contain attachment bytes/,
    );
    expect(() => decodeEnvelope(buildEnvelope(2, "{}", new Uint8Array([1])))).toThrow(
      /must not contain attachment bytes/,
    );
    expect(decodeEnvelope(buildEnvelope(STATUS_ERROR, '{"code":"x"}'))).toMatchObject({
      status: STATUS_ERROR,
      value: { code: "x" },
    });
  });
});

describe("schema-directed attachments", () => {
  it("materializes every dtype as an owned typed-array copy", () => {
    for (const { dt, ctor, make } of DTYPE_CASES) {
      const original = make();
      const tail = new Uint8Array(original.buffer.slice(0));
      const result = bufferFromEnvelope(placeholder(0, original.length, dt), tail, dt);
      expect(result, dt).toBeInstanceOf(ctor);
      expect(valuesOf(result), dt).toEqual(valuesOf(original));
      tail.fill(0xff);
      expect(valuesOf(result), dt).toEqual(valuesOf(original));
    }
  });

  it("copies correctly when the envelope tail view begins at an unaligned backing offset", () => {
    const data = new Float64Array([1.5, -2.5]);
    let value: Record<string, unknown> = { buf: placeholder(0, 2, "f64"), pad: "" };
    if ((HEADER_LEN + JSON.stringify(value).length) % 2 === 0) value = { ...value, pad: "x" };
    const decoded = decodeEnvelope(
      buildEnvelope(0, JSON.stringify(value), new Uint8Array(data.buffer)),
    );
    expect(decoded.tail.byteOffset % 2).toBe(1);
    expect(
      valuesOf(
        bufferFromEnvelope((decoded.value as { buf: unknown }).buf, decoded.tail, "f64"),
      ),
    ).toEqual([1.5, -2.5]);
  });

  it("validates exact wrapper shape, dtype, bounds, arithmetic, and alignment", () => {
    const tail = new Uint8Array(16);
    expect(() => bufferFromEnvelope({ ...placeholder(0, 1, "u8"), sibling: true }, tail, "u8"))
      .toThrow(/attachment wrapper/);
    expect(() => bufferFromEnvelope(placeholder(0, 1, "i8"), tail, "u8"))
      .toThrow(/expected a u8.*got i8/);
    expect(() => bufferFromEnvelope(placeholder(12, 2, "f64"), tail, "f64"))
      .toThrow(/exceeds tail/);
    expect(() => bufferFromEnvelope(placeholder(2, 1, "f64"), tail, "f64"))
      .toThrow(/not aligned/);
    expect(() => bufferFromEnvelope(placeholder(0, Number.MAX_SAFE_INTEGER, "f64"), tail, "f64"))
      .toThrow(/exceeds tail/);
    expect(() => bufferFromEnvelope(placeholder(0, 1, "wat" as Dtype), tail, "u8"))
      .toThrow(/malformed buffer placeholder/);
  });

  it("supports zero-length and large attachments", () => {
    expect(bufferFromEnvelope(placeholder(0, 0, "f64"), new Uint8Array(), "f64"))
      .toBeInstanceOf(Float64Array);
    const count = 131072;
    const data = new Float64Array(count);
    for (let index = 0; index < count; index++) data[index] = index * 0.25;
    const result = bufferFromEnvelope(
      placeholder(0, count, "f64"),
      new Uint8Array(data.buffer),
      "f64",
    ) as Float64Array;
    expect(result.length).toBe(count);
    expect(result[65536]).toBe(65536 * 0.25);
  });
});
