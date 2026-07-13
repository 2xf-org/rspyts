/**
 * Decoding of the rspyts response envelope (ABI §4) and substitution of
 * `__rspyts_buf__` tail placeholders with typed arrays (ABI §6).
 *
 * Every bridged function returns a pointer to a single allocation with a
 * 12-byte little-endian header:
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
 * This module operates on envelope bytes already copied out of WASM linear
 * memory; reading from (and freeing inside) the module is `call.ts`'s job.
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
export type Dtype = "u8" | "i16" | "i32" | "f32" | "f64";

/** The typed arrays that raw buffers and slices materialize as. */
export type BufTypedArray = Uint8Array | Int16Array | Int32Array | Float32Array | Float64Array;

interface DtypeInfo {
  readonly ctor: new (buffer: ArrayBuffer) => BufTypedArray;
  /** Element size in bytes; also the natural alignment of the element. */
  readonly bytes: number;
}

/** Per-dtype typed-array constructor and element size. */
export const DTYPE: Record<Dtype, DtypeInfo> = {
  u8: { ctor: Uint8Array, bytes: 1 },
  i16: { ctor: Int16Array, bytes: 2 },
  i32: { ctor: Int32Array, bytes: 4 },
  f32: { ctor: Float32Array, bytes: 4 },
  f64: { ctor: Float64Array, bytes: 8 },
};

/**
 * The JSON key marking a tail-buffer placeholder. Mirrors
 * `BUF_PLACEHOLDER_KEY` in `rspyts-core`; never changes within an ABI
 * major version.
 */
export const BUF_PLACEHOLDER_KEY = "__rspyts_buf__";

const utf8Decoder = new TextDecoder();

/** A decoded envelope: `payload` is the parsed JSON, with every buffer
 * placeholder already replaced by a typed array when `status` is ok. */
export interface DecodedEnvelope {
  status: number;
  payload: unknown;
}

/**
 * Decode a complete envelope. Placeholders are substituted only for
 * status-ok payloads — error payloads never carry them (their tail is
 * always empty, ABI §4/§5).
 */
export function decodeEnvelope(bytes: Uint8Array): DecodedEnvelope {
  if (bytes.byteLength < HEADER_LEN) {
    throw new Error(`rspyts: truncated envelope: ${bytes.byteLength} bytes, need ${HEADER_LEN}`);
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const status = view.getUint8(0);
  const jsonLen = view.getUint32(4, true);
  const tailLen = view.getUint32(8, true);
  if (HEADER_LEN + jsonLen + tailLen > bytes.byteLength) {
    throw new Error(
      `rspyts: truncated envelope: header promises ${HEADER_LEN + jsonLen + tailLen} bytes, got ${bytes.byteLength}`,
    );
  }
  const json = bytes.subarray(HEADER_LEN, HEADER_LEN + jsonLen);
  const tail = bytes.subarray(HEADER_LEN + jsonLen, HEADER_LEN + jsonLen + tailLen);
  const payload: unknown = JSON.parse(utf8Decoder.decode(json));
  return {
    status,
    payload: status === STATUS_OK ? substituteBuffers(payload, tail) : payload,
  };
}

/**
 * Recursively replace every `{"__rspyts_buf__": {off, len, dt}}`
 * placeholder in `value` with a typed array holding the corresponding
 * elements of `tail`. Only objects whose SOLE key is the placeholder key
 * are substituted; anything else is walked structurally.
 *
 * The result is always a copy: `tail` is a view whose `byteOffset` need
 * not be a multiple of the element size (the tail merely follows the JSON
 * bytes inside the envelope), so constructing a typed array directly over
 * `tail.buffer` could throw on alignment. Copying the bytes into a fresh
 * buffer first sidesteps alignment entirely and guarantees the returned
 * array is independent of the envelope's lifetime.
 */
export function substituteBuffers(value: unknown, tail: Uint8Array): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => substituteBuffers(item, tail));
  }
  if (value !== null && typeof value === "object") {
    const obj = value as Record<string, unknown>;
    const keys = Object.keys(obj);
    if (keys.length === 1 && keys[0] === BUF_PLACEHOLDER_KEY) {
      return materializeBuf(obj[BUF_PLACEHOLDER_KEY], tail);
    }
    const out: Record<string, unknown> = {};
    for (const key of keys) {
      out[key] = substituteBuffers(obj[key], tail);
    }
    return out;
  }
  return value;
}

function materializeBuf(body: unknown, tail: Uint8Array): BufTypedArray {
  if (body === null || typeof body !== "object") {
    throw new Error("rspyts: malformed buffer placeholder: body is not an object");
  }
  const { off, len, dt } = body as { off?: unknown; len?: unknown; dt?: unknown };
  if (
    typeof off !== "number" ||
    typeof len !== "number" ||
    typeof dt !== "string" ||
    !Object.prototype.hasOwnProperty.call(DTYPE, dt)
  ) {
    throw new Error(`rspyts: malformed buffer placeholder: ${JSON.stringify(body)}`);
  }
  const info = DTYPE[dt as Dtype];
  const byteLen = len * info.bytes;
  if (off < 0 || off + byteLen > tail.byteLength) {
    throw new Error(
      `rspyts: buffer placeholder (off ${off}, ${byteLen} bytes) exceeds tail of ${tail.byteLength} bytes`,
    );
  }
  // Uint8Array.prototype.slice yields a fresh, offset-0, correctly-sized
  // buffer — safe to wrap in any typed-array constructor.
  const copied = tail.slice(off, off + byteLen);
  return new info.ctor(copied.buffer);
}
