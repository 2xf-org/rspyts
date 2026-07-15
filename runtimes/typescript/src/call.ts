/**
 * The calling convention (ABI §2, §3.1): marshal arguments into WASM
 * linear memory, invoke a bridged export, and decode its response
 * envelope.
 *
 * Two invariants shape everything here:
 *
 * 1. **Growth safety.** Any `rspyts_alloc` call — and the bridged call
 *    itself — may grow linear memory, which detaches every existing
 *    `ArrayBuffer` view. Views over `mod.memory.buffer` are therefore
 *    created immediately before use and never retained across an
 *    allocation or a call.
 * 2. **Exact-length frees.** Every allocation is freed with
 *    `rspyts_free(ptr, len)` using the exact originally-allocated length
 *    (ABI §2), including the over-allocated length of aligned slice
 *    buffers and the header-derived total length of envelopes.
 */

import {
  DTYPE,
  STATUS_OK,
  bufferFromEnvelope,
  buildRequest,
  checkedJsonValue,
  decodeEnvelope,
  type BufTypedArray,
  type Dtype,
} from "./envelope.js";
import {
  type BridgeErrorRegistry,
  throwBridgeError,
} from "./errors.js";
import {
  allocRaw,
  ensureHealthy,
  ensureMemoryRange,
  exportFn,
  freeRaw,
  invokeWasm,
  isPoisoned,
  takeOwnedEnvelope,
  type BridgeModule,
} from "./module.js";

const I64_MIN = -(1n << 63n);
const I64_MAX = (1n << 63n) - 1n;
const U64_MAX = (1n << 64n) - 1n;
const MAX_HANDLE = BigInt(Number.MAX_SAFE_INTEGER);

function checkedHandle(handle: unknown): bigint {
  if (typeof handle !== "bigint") {
    throw new TypeError("rspyts: class handle must be a bigint");
  }
  if (handle < 1n || handle > MAX_HANDLE) {
    throw new RangeError("rspyts: class handle must be between 1 and Number.MAX_SAFE_INTEGER");
  }
  return handle;
}

/**
 * One successful ABI-3 response position. `tail` is explicit and owned host
 * memory; generated schema decoders preserve it while selecting child values.
 */
export interface WireResponse<Value = unknown> {
  readonly value: Value;
  readonly tail: Uint8Array;
}

/** Create an explicit response context, primarily for attachment-free error data. */
export function wireResponse<Value>(
  value: Value,
  tail: Uint8Array = new Uint8Array(),
): WireResponse<Value> {
  return { value, tail };
}

function childResponse<Value>(parent: WireResponse, value: Value): WireResponse<Value> {
  return { value, tail: parent.tail };
}

function exactWireInteger(value: unknown, signed: boolean): bigint {
  const label = signed ? "signed" : "unsigned";
  if (typeof value !== "string") {
    throw new TypeError(`rspyts: expected a canonical ${label} 64-bit decimal string`);
  }
  const canonical = signed ? /^(?:0|-?[1-9][0-9]*)$/ : /^(?:0|[1-9][0-9]*)$/;
  if (!canonical.test(value)) {
    throw new TypeError(`rspyts: expected a canonical ${label} 64-bit decimal string`);
  }
  const parsed = BigInt(value);
  const minimum = signed ? I64_MIN : 0n;
  const maximum = signed ? I64_MAX : U64_MAX;
  if (parsed < minimum || parsed > maximum) {
    throw new RangeError(`rspyts: ${signed ? "i64" : "u64"} value out of range: ${value}`);
  }
  return parsed;
}

function exactHostInteger(value: bigint, signed: boolean): string {
  if (typeof value !== "bigint") {
    throw new TypeError(`rspyts: expected a ${signed ? "signed" : "unsigned"} 64-bit bigint`);
  }
  const minimum = signed ? I64_MIN : 0n;
  const maximum = signed ? I64_MAX : U64_MAX;
  if (value < minimum || value > maximum) {
    throw new RangeError(`rspyts: ${signed ? "i64" : "u64"} value out of range: ${value}`);
  }
  return value.toString(10);
}

/** Convert a host bigint to the canonical signed 64-bit wire string. */
export function i64ToWire(value: bigint): string {
  return exactHostInteger(value, true);
}

/** Convert a host bigint to the canonical unsigned 64-bit wire string. */
export function u64ToWire(value: bigint): string {
  return exactHostInteger(value, false);
}

/** Validate a canonical signed 64-bit wire string and return a bigint. */
export function i64FromWire(response: WireResponse): bigint {
  return exactWireInteger(response.value, true);
}

/** Validate a canonical unsigned 64-bit wire string and return a bigint. */
export function u64FromWire(response: WireResponse): bigint {
  return exactWireInteger(response.value, false);
}

/** Validate a finite structured float and canonicalize signed zero. */
export function floatFromWire(response: WireResponse): number {
  const value = response.value;
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError("rspyts: expected a finite JSON number");
  }
  return Object.is(value, -0) ? 0 : value;
}

/** Validate a finite structured f32 without accepting values Rust cannot represent. */
export function f32FromWire(response: WireResponse): number {
  const parsed = floatFromWire(response);
  if (!Number.isFinite(Math.fround(parsed))) {
    throw new RangeError(`rspyts: f32 value out of range: ${parsed}`);
  }
  return parsed;
}

/** Validate an exact JSON boolean. */
export function boolFromWire(response: WireResponse): boolean {
  const value = response.value;
  if (typeof value !== "boolean") {
    throw new TypeError("rspyts: expected a JSON boolean");
  }
  return value;
}

/** Validate an exact, bounded JSON integer. */
export function boundedIntFromWire(
  response: WireResponse,
  minimum: number,
  maximum: number,
): number {
  const value = response.value;
  if (typeof value !== "number" || !Number.isInteger(value)) {
    throw new TypeError("rspyts: expected a JSON integer");
  }
  if (value < minimum || value > maximum) {
    throw new RangeError(`rspyts: integer value out of range ${minimum}..=${maximum}: ${value}`);
  }
  return value;
}

/** Validate an exact JSON string. */
export function stringFromWire(response: WireResponse): string {
  const value = response.value;
  if (typeof value !== "string") {
    throw new TypeError("rspyts: expected a JSON string");
  }
  return value;
}

/** Validate a materialized opaque-bytes attachment. */
export function bytesFromWire(response: WireResponse): Uint8Array {
  return bufferFromEnvelope(response.value, response.tail, "bytes") as Uint8Array;
}

/** Validate a materialized numeric-buffer attachment and its declared dtype. */
export function bufferFromWire(response: WireResponse, dtype: Dtype): BufTypedArray {
  return bufferFromEnvelope(response.value, response.tail, dtype);
}

/** Validate an exact JSON array. */
export function listFromWire(response: WireResponse): WireResponse[] {
  const value = response.value;
  if (!Array.isArray(value)) {
    throw new TypeError("rspyts: expected a JSON array");
  }
  return value.map((item) => childResponse(response, item));
}

/** Validate an exact JSON object. */
export function mapFromWire(response: WireResponse): Record<string, WireResponse> {
  const value = response.value;
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError("rspyts: expected a JSON object");
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    throw new TypeError("rspyts: expected a plain JSON object");
  }
  const result = Object.create(null) as Record<string, WireResponse>;
  for (const [key, item] of Object.entries(value)) {
    result[key] = childResponse(response, item);
  }
  return result;
}

/** Validate the exact field set of one generated struct or enum variant. */
export function objectFromWire(
  response: WireResponse,
  required: readonly string[],
  optional: readonly string[] = [],
): Record<string, WireResponse> {
  const object = mapFromWire(response);
  const requiredSet = new Set(required);
  const allowed = new Set([...required, ...optional]);
  for (const key of requiredSet) {
    if (!Object.prototype.hasOwnProperty.call(object, key)) {
      throw new TypeError(`rspyts: expected required wire field ${JSON.stringify(key)}`);
    }
  }
  for (const key of Object.keys(object)) {
    if (!allowed.has(key)) {
      throw new TypeError(`rspyts: unexpected wire field ${JSON.stringify(key)}`);
    }
  }
  return object;
}

/** Validate the JSON-array representation of a fixed-length tuple. */
export function tupleFromWire(response: WireResponse, length: number): WireResponse[] {
  const items = listFromWire(response);
  if (items.length !== length) {
    throw new RangeError(`rspyts: expected a JSON array of length ${length}, got ${items.length}`);
  }
  return items;
}

/** Validate one generated string-enum wire value. */
export function stringEnumFromWire<const Value extends string>(
  response: WireResponse,
  variants: readonly Value[],
): Value {
  const string = stringFromWire(response);
  if (!(variants as readonly string[]).includes(string)) {
    throw new TypeError(`rspyts: unknown string-enum wire value ${JSON.stringify(string)}`);
  }
  return string as Value;
}

/** Field requirements for one internally tagged data-enum variant. */
export interface WireVariantShape {
  readonly required: readonly string[];
  readonly optional?: readonly string[];
}

/** Validate and dispatch one internally tagged data-enum wire object. */
export function enumFromWire(
  response: WireResponse,
  tag: string,
  variants: Readonly<Record<string, WireVariantShape>>,
): Record<string, WireResponse> {
  const candidate = mapFromWire(response);
  if (!Object.prototype.hasOwnProperty.call(candidate, tag)) {
    throw new TypeError(`rspyts: expected required enum tag ${JSON.stringify(tag)}`);
  }
  const discriminator = stringFromWire(candidate[tag]!);
  if (!Object.prototype.hasOwnProperty.call(variants, discriminator)) {
    throw new TypeError(
      `rspyts: unknown ${JSON.stringify(tag)} discriminator ${JSON.stringify(discriminator)}`,
    );
  }
  const shape = variants[discriminator];
  // The own-property check above proves this generated table entry exists.
  if (shape === undefined) throw new TypeError("rspyts: invalid enum variant table");
  return objectFromWire(response, [tag, ...shape.required], shape.optional ?? []);
}

/** Validate the one-value null data type. */
export function nullFromWire(response: WireResponse): null {
  if (response.value !== null) {
    throw new TypeError("rspyts: expected null");
  }
  return null;
}

/** Validate and copy a transparent value at a schema-declared `Json` position. */
export function jsonFromWire(response: WireResponse): unknown {
  return checkedJsonValue(response.value);
}

/**
 * One `&[T]` slice argument: the data to pass and its wire dtype. The
 * typed array's constructor must match `dt` (`Float64Array` for `"f64"`,
 * etc.); plain arrays are deliberately not accepted.
 */
export interface SliceArg {
  data: BufTypedArray;
  dt: Dtype;
}

/** A live `rspyts_alloc` allocation: free with exactly this (ptr, len). */
interface Allocation {
  ptr: number;
  len: number;
}

interface SliceAllocation extends Allocation {
  /** Pointer actually passed to the call: `ptr` rounded up to the
   * element's natural alignment. */
  aligned: number;
  /** Element count (not bytes) — the `len` half of the C (ptr, len) pair. */
  count: number;
}

/**
 * Copy `bytes` into a fresh `rspyts_alloc` allocation. The memory view is
 * created only after the allocation returns: growing linear memory
 * detaches every earlier view.
 */
export function writeBytes(mod: BridgeModule, bytes: Uint8Array): Allocation {
  const owned = new Uint8Array(bytes);
  const len = owned.byteLength;
  const ptr = allocRaw(mod, len);
  if (len > 0) {
    ensureMemoryRange(mod, ptr, len, "request allocation");
    new Uint8Array(mod.memory.buffer, ptr, len).set(owned);
  }
  return { ptr, len };
}

/**
 * Copy a slice argument into WASM memory at the natural alignment of its
 * element type (ABI §6).
 *
 * `rspyts_alloc` guarantees alignment 1 only, but slice pointers must be
 * naturally aligned. We cannot simply round the returned pointer up,
 * because `rspyts_free` must later be called with the ORIGINAL (ptr, len)
 * pair. So: over-allocate by `align` bytes, write the elements at the
 * first aligned offset inside the allocation, hand that aligned pointer
 * to the call, and free the original allocation afterwards.
 */
export function writeSlice(mod: BridgeModule, slice: SliceArg): SliceAllocation {
  const info = DTYPE[slice.dt];
  if (!(slice.data instanceof info.ctor)) {
    throw new TypeError(
      `rspyts: slice dtype "${slice.dt}" requires a ${info.ctor.name}, ` +
        `got ${slice.data.constructor.name}`,
    );
  }
  const byteLen = slice.data.byteLength;
  // Encode before allocating so a detached input or a memory-growing alloc
  // cannot strand an allocation. Typed-array backing bytes are host-endian;
  // DataView writes make the ABI's little-endian requirement explicit on
  // every host.
  const wire = new Uint8Array(byteLen);
  const wireView = new DataView(wire.buffer);
  for (let index = 0; index < slice.data.length; index++) {
    const element = slice.data[index];
    if (element === undefined) {
      throw new Error("rspyts: typed array changed length while encoding");
    }
    info.write(wireView, index * info.bytes, element);
  }
  const align = info.bytes;
  const len = byteLen + align;
  const ptr = allocRaw(mod, len);
  const aligned = ptr + ((align - (ptr % align)) % align);
  ensureMemoryRange(mod, aligned, byteLen, "slice allocation");
  new Uint8Array(mod.memory.buffer, aligned, byteLen).set(wire);
  return { ptr, len, aligned, count: slice.data.length };
}

/**
 * Invoke one ABI-3 generated export. This is the sole function-call path.
 * Successful calls return explicit `{ value, tail }` context for the
 * generated schema decoder; application errors and panics throw.
 */
export function callFn(
  mod: BridgeModule,
  symbol: string,
  args: unknown,
  slices: SliceArg[] = [],
  handle?: bigint,
  errorTypes?: BridgeErrorRegistry,
): WireResponse {
  ensureHealthy(mod);
  const methodHandle = handle === undefined ? undefined : checkedHandle(handle);
  const fn = exportFn(mod, symbol);
  const allocations: Allocation[] = [];
  let envelope: Uint8Array;
  try {
    const argsAlloc = writeBytes(mod, buildRequest(args ?? {}));
    allocations.push(argsAlloc);

    const callArgs: Array<number | bigint> = methodHandle === undefined ? [] : [methodHandle];
    callArgs.push(argsAlloc.ptr, argsAlloc.len);
    for (const slice of slices) {
      const sliceAlloc = writeSlice(mod, slice);
      allocations.push(sliceAlloc);
      callArgs.push(sliceAlloc.aligned, sliceAlloc.count);
    }

    envelope = takeOwnedEnvelope(mod, symbol, invokeWasm(mod, fn, callArgs));
  } finally {
    // Request buffers stay alive for the duration of the call (the callee
    // borrows, never frees — ABI §2) and are released even when the call
    // traps.
    for (const allocation of allocations) {
      if (isPoisoned(mod)) {
        break;
      }
      freeRaw(mod, allocation.ptr, allocation.len);
    }
  }

  const { status, value, tail } = decodeEnvelope(envelope);
  if (status === STATUS_OK) {
    return { value, tail };
  }
  throwBridgeError(status, value, errorTypes);
}

/**
 * Invoke a `__drop` export (ABI §8). Fire-and-forget: `__drop` is
 * idempotent and infallible by contract, and this is called from
 * `free()`, `[Symbol.dispose]`, and `FinalizationRegistry` callbacks,
 * where a throw would be unactionable — so a trapped instance is
 * swallowed. A missing export (a codegen bug) still throws.
 */
export function callDrop(mod: BridgeModule, symbol: string, handle: bigint): void {
  const classHandle = checkedHandle(handle);
  if (isPoisoned(mod)) {
    return;
  }
  const fn = exportFn(mod, symbol);
  try {
    invokeWasm(mod, fn, [classHandle]);
  } catch {
    // Deliberately ignored; see doc comment.
  }
}
