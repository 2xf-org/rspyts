/**
 * Strict ABI-3 request/response envelope codecs (ABI §4).
 *
 * Both directions use one 12-byte little-endian header. Byte zero is a
 * zero request marker on calls and a closed 0/1/2 status on responses:
 *
 * ```text
 * offset  size       field
 * 0       1          status   (0 ok, 1 error, 2 panic)
 * 1       3          reserved (zero)
 * 4       4          json_len (u32 LE)
 * 8       4          tail_len (u32 LE)
 * 12      json_len   UTF-8 JSON payload
 * 12+j    tail_len   raw numeric tail (Buf<T> data)
 * ```
 *
 * This module operates on owned host bytes; copying to/from WASM linear
 * memory and freeing module allocations is `call.ts`'s job.
 */

/** Byte length of the envelope header (ABI §4). */
export const HEADER_LEN = 12;

/** Envelope status: successful return value. */
export const STATUS_OK = 0;
/** Envelope status: application error (the bridged function's `Err`). */
export const STATUS_ERROR = 1;
/** Envelope status: a panic was caught behind the shim. */
export const STATUS_PANIC = 2;

/** Wire names of the supported raw-buffer element types (ABI §6). */
export type Dtype =
  | "u8"
  | "i8"
  | "u16"
  | "i16"
  | "u32"
  | "i32"
  | "u64"
  | "i64"
  | "f32"
  | "f64";

type AttachmentDtype = Dtype | "bytes";

/** The typed arrays that raw buffers and slices materialize as. */
export type BufTypedArray =
  | Uint8Array
  | Int8Array
  | Uint16Array
  | Int16Array
  | Uint32Array
  | Int32Array
  | BigUint64Array
  | BigInt64Array
  | Float32Array
  | Float64Array;

interface DtypeInfo {
  readonly ctor: new (buffer: ArrayBuffer) => BufTypedArray;
  /** Element size in bytes; also the natural alignment of the element. */
  readonly bytes: number;
  readonly create: (length: number) => BufTypedArray;
  readonly read: (view: DataView, offset: number) => number | bigint;
  readonly write: (view: DataView, offset: number, value: number | bigint) => void;
}

/** Per-dtype typed-array constructor and element size. */
export const DTYPE: Record<Dtype, DtypeInfo> = {
  u8: {
    ctor: Uint8Array,
    bytes: 1,
    create: (n) => new Uint8Array(n),
    read: (view, off) => view.getUint8(off),
    write: (view, off, value) => view.setUint8(off, Number(value)),
  },
  i8: {
    ctor: Int8Array,
    bytes: 1,
    create: (n) => new Int8Array(n),
    read: (view, off) => view.getInt8(off),
    write: (view, off, value) => view.setInt8(off, Number(value)),
  },
  u16: {
    ctor: Uint16Array,
    bytes: 2,
    create: (n) => new Uint16Array(n),
    read: (view, off) => view.getUint16(off, true),
    write: (view, off, value) => view.setUint16(off, Number(value), true),
  },
  i16: {
    ctor: Int16Array,
    bytes: 2,
    create: (n) => new Int16Array(n),
    read: (view, off) => view.getInt16(off, true),
    write: (view, off, value) => view.setInt16(off, Number(value), true),
  },
  u32: {
    ctor: Uint32Array,
    bytes: 4,
    create: (n) => new Uint32Array(n),
    read: (view, off) => view.getUint32(off, true),
    write: (view, off, value) => view.setUint32(off, Number(value), true),
  },
  i32: {
    ctor: Int32Array,
    bytes: 4,
    create: (n) => new Int32Array(n),
    read: (view, off) => view.getInt32(off, true),
    write: (view, off, value) => view.setInt32(off, Number(value), true),
  },
  u64: {
    ctor: BigUint64Array,
    bytes: 8,
    create: (n) => new BigUint64Array(n),
    read: (view, off) => view.getBigUint64(off, true),
    write: (view, off, value) => view.setBigUint64(off, BigInt(value), true),
  },
  i64: {
    ctor: BigInt64Array,
    bytes: 8,
    create: (n) => new BigInt64Array(n),
    read: (view, off) => view.getBigInt64(off, true),
    write: (view, off, value) => view.setBigInt64(off, BigInt(value), true),
  },
  f32: {
    ctor: Float32Array,
    bytes: 4,
    create: (n) => new Float32Array(n),
    read: (view, off) => view.getFloat32(off, true),
    write: (view, off, value) => view.setFloat32(off, Number(value), true),
  },
  f64: {
    ctor: Float64Array,
    bytes: 8,
    create: (n) => new Float64Array(n),
    read: (view, off) => view.getFloat64(off, true),
    write: (view, off, value) => view.setFloat64(off, Number(value), true),
  },
};

/**
 * The JSON key marking a tail-buffer placeholder. Mirrors
 * `BUF_PLACEHOLDER_KEY` in `rspyts-core`; never changes within an ABI
 * major version.
 */
export const BUF_PLACEHOLDER_KEY = "__rspyts_buf__";

const utf8Decoder = new TextDecoder("utf-8", { fatal: true });
const utf8Encoder = new TextEncoder();
const WIRE_BUFFER_MARKER: unique symbol = Symbol("rspyts.wireBuffer");

/** Explicit marker for a numeric typed array nested in request JSON. */
export interface WireBuffer {
  readonly [WIRE_BUFFER_MARKER]: true;
  readonly data: BufTypedArray;
  readonly dt: Dtype;
}

/** Mark a typed array as a numeric `Buf<T>` request attachment. */
export function wireBuffer(data: BufTypedArray, dt: Dtype): WireBuffer {
  const info = DTYPE[dt];
  if (!(data instanceof info.ctor)) {
    throw new TypeError(
      `rspyts: buffer dtype "${dt}" requires a ${info.ctor.name}, got ${data.constructor.name}`,
    );
  }
  return Object.freeze({ [WIRE_BUFFER_MARKER]: true as const, data, dt });
}

/** Validate and copy a transparent schema-declared JSON request value. */
export function jsonToWire(value: unknown): unknown {
  return checkedJsonValue(value);
}

/** Encode JSON plus aligned attachments as a strict ABI-3 request. */
export function buildRequest(args: unknown): Uint8Array {
  const chunks: Uint8Array[] = [];
  let tailLen = 0;

  const encode = (value: unknown): unknown => {
    if (isWireBuffer(value)) {
      const info = DTYPE[value.dt];
      if (!(value.data instanceof info.ctor)) {
        throw new TypeError(`rspyts: wire buffer data does not match dtype "${value.dt}"`);
      }
      const pad = (info.bytes - (tailLen % info.bytes)) % info.bytes;
      if (pad > 0) {
        chunks.push(new Uint8Array(pad));
        tailLen += pad;
      }
      const off = tailLen;
      const bytes = new Uint8Array(value.data.length * info.bytes);
      const view = new DataView(bytes.buffer);
      for (let i = 0; i < value.data.length; i++) {
        const element = value.data[i];
        if (element === undefined) {
          throw new Error("rspyts: typed array changed length while encoding");
        }
        info.write(view, i * info.bytes, element);
      }
      chunks.push(bytes);
      tailLen += bytes.byteLength;
      return { [BUF_PLACEHOLDER_KEY]: { off, len: value.data.length, dt: value.dt } };
    }
    if (value instanceof Uint8Array) {
      const off = tailLen;
      const bytes = new Uint8Array(value);
      chunks.push(bytes);
      tailLen += bytes.byteLength;
      return { [BUF_PLACEHOLDER_KEY]: { off, len: bytes.byteLength, dt: "bytes" } };
    }
    if (ArrayBuffer.isView(value)) {
      throw new TypeError(
        "rspyts: bare typed arrays are not numeric buffers; use wireBuffer(view, dt)",
      );
    }
    if (Array.isArray(value)) {
      return value.map(encode);
    }
    if (value !== null && typeof value === "object") {
      const output = Object.create(null) as Record<string, unknown>;
      for (const [key, item] of Object.entries(value)) {
        output[key] = encode(item);
      }
      return output;
    }
    if (typeof value === "number") {
      if (!Number.isFinite(value)) {
        throw new TypeError(
          "rspyts: non-finite numbers cannot cross in JSON positions; use a slice or Buf instead",
        );
      }
      return Object.is(value, -0) ? 0 : value;
    }
    return value;
  };

  const jsonText = JSON.stringify(encode(args ?? {}));
  if (jsonText === undefined) {
    throw new TypeError("rspyts: request arguments are not JSON serializable");
  }
  const json = utf8Encoder.encode(jsonText);
  if (json.byteLength > 0xffffffff || tailLen > 0xffffffff) {
    throw new RangeError("rspyts: request envelope component exceeds the 4 GiB limit");
  }
  const request = new Uint8Array(HEADER_LEN + json.byteLength + tailLen);
  const header = new DataView(request.buffer);
  header.setUint32(4, json.byteLength, true);
  header.setUint32(8, tailLen, true);
  request.set(json, HEADER_LEN);
  let cursor = HEADER_LEN + json.byteLength;
  for (const chunk of chunks) {
    request.set(chunk, cursor);
    cursor += chunk.byteLength;
  }
  return request;
}

/** Validate and copy one transparent, schema-declared JSON value. */
export function checkedJsonValue(value: unknown): unknown {
  return checkedJson(value, new Set());
}

function checkedJson(value: unknown, seen: Set<object>): unknown {
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new TypeError("rspyts: Json values must contain only finite JSON numbers");
    }
    if (Number.isInteger(value) && !Number.isSafeInteger(value)) {
      throw new TypeError("rspyts: Json integral numbers must be safe integers");
    }
    return Object.is(value, -0) ? 0 : value;
  }
  if (typeof value !== "object") {
    throw new TypeError(`rspyts: Json contains a non-JSON ${typeof value} value`);
  }
  if (seen.has(value)) {
    throw new TypeError("rspyts: Json values cannot contain cycles");
  }
  seen.add(value);
  try {
    if (Array.isArray(value)) {
      const ownKeys = Reflect.ownKeys(value);
      for (let index = 0; index < value.length; index++) {
        if (!Object.prototype.hasOwnProperty.call(value, index)) {
          throw new TypeError("rspyts: Json arrays cannot contain sparse holes");
        }
      }
      for (const key of ownKeys) {
        if (key === "length") continue;
        if (typeof key !== "string" || !/^(?:0|[1-9][0-9]*)$/.test(key) || Number(key) >= value.length) {
          throw new TypeError("rspyts: Json arrays cannot contain non-element own properties");
        }
        const descriptor = Object.getOwnPropertyDescriptor(value, key)!;
        if (!descriptor.enumerable || !("value" in descriptor)) {
          throw new TypeError("rspyts: Json array elements must be enumerable data properties");
        }
      }
      return value.map((item) => checkedJson(item, seen));
    }
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) {
      throw new TypeError("rspyts: Json objects must be plain objects");
    }
    const output = Object.create(null) as Record<string, unknown>;
    for (const key of Reflect.ownKeys(value)) {
      if (typeof key !== "string") {
        throw new TypeError("rspyts: Json objects cannot contain symbol properties");
      }
      const descriptor = Object.getOwnPropertyDescriptor(value, key)!;
      if (!descriptor.enumerable || !("value" in descriptor)) {
        throw new TypeError("rspyts: Json object fields must be enumerable data properties");
      }
      output[key] = checkedJson(descriptor.value, seen);
    }
    return output;
  } finally {
    seen.delete(value);
  }
}

function isWireBuffer(value: unknown): value is WireBuffer {
  const candidate = value as Partial<WireBuffer> | null;
  return (
    candidate !== null &&
    typeof candidate === "object" &&
    candidate[WIRE_BUFFER_MARKER] === true &&
    typeof candidate.dt === "string" &&
    Object.prototype.hasOwnProperty.call(DTYPE, candidate.dt)
  );
}

/** A validated response status, parsed JSON value, and owned tail bytes. */
export interface DecodedEnvelope {
  status: number;
  value: unknown;
  tail: Uint8Array;
}

/** A validated request, retained in wire form for conformance diagnostics. */
export interface DecodedRequest {
  value: unknown;
  tail: Uint8Array;
}

/** Decode ABI-3 request framing and JSON without interpreting object shapes. */
export function decodeRequest(bytes: Uint8Array): DecodedRequest {
  const decoded = decodeFrame(bytes, true);
  return { value: decoded.value, tail: decoded.tail };
}

/**
 * Decode a complete ABI-3 response without interpreting schema-directed
 * attachment shapes. Error and panic responses must have an empty tail.
 */
export function decodeEnvelope(bytes: Uint8Array): DecodedEnvelope {
  const decoded = decodeFrame(bytes, false);
  if (decoded.status !== STATUS_OK) {
    if (decoded.tail.byteLength !== 0) {
      throw new Error("rspyts: error and panic envelopes must not contain attachment bytes");
    }
  }
  return decoded;
}

interface RawEnvelope {
  status: number;
  value: unknown;
  tail: Uint8Array;
}

function decodeFrame(bytes: Uint8Array, request: boolean): RawEnvelope {
  if (bytes.byteLength < HEADER_LEN) {
    throw new Error(
      `rspyts: truncated envelope header: expected ${HEADER_LEN} bytes, got ${bytes.byteLength}`,
    );
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const status = view.getUint8(0);
  if (request && status !== 0) {
    throw new Error(`rspyts: invalid request marker ${status}; expected 0`);
  }
  if (!request && status !== STATUS_OK && status !== STATUS_ERROR && status !== STATUS_PANIC) {
    throw new Error(`rspyts: invalid response status ${status}; expected 0, 1, or 2`);
  }
  if (view.getUint8(1) !== 0 || view.getUint8(2) !== 0 || view.getUint8(3) !== 0) {
    throw new Error("rspyts: reserved envelope header bytes must be zero");
  }
  const jsonLen = view.getUint32(4, true);
  const tailLen = view.getUint32(8, true);
  const total = HEADER_LEN + jsonLen + tailLen;
  if (total > bytes.byteLength) {
    throw new Error(
      `rspyts: truncated envelope: header declares ${total} bytes, got ${bytes.byteLength}`,
    );
  }
  if (total < bytes.byteLength) {
    throw new Error(
      `rspyts: trailing bytes after envelope: header declares ${total} bytes, got ${bytes.byteLength}`,
    );
  }
  const json = bytes.subarray(HEADER_LEN, HEADER_LEN + jsonLen);
  const tail = bytes.subarray(HEADER_LEN + jsonLen, total);
  let value: unknown;
  try {
    value = JSON.parse(utf8Decoder.decode(json));
  } catch (error) {
    throw new Error(`rspyts: malformed envelope JSON: ${String(error)}`, { cause: error });
  }
  return { status, value, tail };
}

function validateBufPlaceholder(value: unknown, tail: Uint8Array): CheckedBuffer {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError("rspyts: expected a buffer attachment wrapper");
  }
  const object = value as Record<string, unknown>;
  const keys = Object.keys(object);
  if (keys.length !== 1 || !Object.prototype.hasOwnProperty.call(object, BUF_PLACEHOLDER_KEY)) {
    throw new TypeError("rspyts: expected a buffer attachment wrapper");
  }
  return validateBuf(object[BUF_PLACEHOLDER_KEY], tail);
}

/** Materialize a buffer only at a generated schema-declared attachment position. */
export function bufferFromEnvelope(
  value: unknown,
  tail: Uint8Array,
  expected: AttachmentDtype,
): BufTypedArray {
  const checked = validateBufPlaceholder(value, tail);
  if (checked.dt !== expected) {
    throw new TypeError(
      `rspyts: expected a ${expected} buffer attachment, got ${checked.dt}`,
    );
  }
  return materializeBuf(checked, tail);
}

interface CheckedBuffer {
  off: number;
  len: number;
  dt: AttachmentDtype;
  info: DtypeInfo;
}

const BYTES_INFO: DtypeInfo = {
  ctor: Uint8Array,
  bytes: 1,
  create: (n) => new Uint8Array(n),
  read: (view, off) => view.getUint8(off),
  write: (view, off, value) => view.setUint8(off, Number(value)),
};

function validateBuf(body: unknown, tail: Uint8Array): CheckedBuffer {
  if (body === null || typeof body !== "object") {
    throw new Error("rspyts: malformed buffer placeholder: body is not an object");
  }
  const keys = Object.keys(body);
  const { off, len, dt } = body as { off?: unknown; len?: unknown; dt?: unknown };
  if (
    keys.length !== 3 ||
    !keys.includes("off") ||
    !keys.includes("len") ||
    !keys.includes("dt") ||
    typeof off !== "number" ||
    typeof len !== "number" ||
    typeof dt !== "string" ||
    !Number.isSafeInteger(off) ||
    !Number.isSafeInteger(len) ||
    (dt !== "bytes" && !Object.prototype.hasOwnProperty.call(DTYPE, dt))
  ) {
    throw new Error(`rspyts: malformed buffer placeholder: ${JSON.stringify(body)}`);
  }
  const info = dt === "bytes" ? BYTES_INFO : DTYPE[dt as Dtype];
  const byteLen = len * info.bytes;
  if (off < 0 || len < 0 || !Number.isSafeInteger(byteLen) || off + byteLen > tail.byteLength) {
    throw new Error(
      `rspyts: buffer placeholder (off ${off}, ${byteLen} bytes) exceeds tail of ${tail.byteLength} bytes`,
    );
  }
  if (off % info.bytes !== 0) {
    throw new Error(
      `rspyts: buffer offset ${off} is not aligned to ${info.bytes} bytes for ${dt}`,
    );
  }
  return { off, len, dt: dt as AttachmentDtype, info };
}

function materializeBuf(buffer: CheckedBuffer, tail: Uint8Array): BufTypedArray {
  const output = buffer.info.create(buffer.len);
  const view = new DataView(tail.buffer, tail.byteOffset, tail.byteLength);
  for (let i = 0; i < buffer.len; i++) {
    const value = buffer.info.read(view, buffer.off + i * buffer.info.bytes);
    setElement(output, i, value);
  }
  return output;
}

function setElement(array: BufTypedArray, index: number, value: number | bigint): void {
  if (array instanceof BigInt64Array || array instanceof BigUint64Array) {
    array[index] = value as bigint;
  } else {
    array[index] = value as number;
  }
}
