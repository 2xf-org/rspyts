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
  HEADER_LEN,
  STATUS_OK,
  buildRequest,
  decodeEnvelope,
  type BufTypedArray,
  type Dtype,
} from "./envelope.js";
import {
  type BridgeErrorRegistry,
  InstancePoisonedError,
  throwBridgeError,
} from "./errors.js";
import { isPoisoned, markPoisoned, type BridgeModule } from "./module.js";

const I64_MIN = -(1n << 63n);
const I64_MAX = (1n << 63n) - 1n;
const U64_MAX = (1n << 64n) - 1n;

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
export function i64FromWire(value: unknown): bigint {
  return exactWireInteger(value, true);
}

/** Validate a canonical unsigned 64-bit wire string and return a bigint. */
export function u64FromWire(value: unknown): bigint {
  return exactWireInteger(value, false);
}

/** Validate a finite structured float and canonicalize signed zero. */
export function floatFromWire(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError("rspyts: expected a finite JSON number");
  }
  return Object.is(value, -0) ? 0 : value;
}

/** Unwrap a schemaless JSON field from an opaque application-error payload. */
export function jsonFromWire(value: unknown): unknown {
  if (
    value === null ||
    typeof value !== "object" ||
    Object.keys(value).length !== 1 ||
    !Object.prototype.hasOwnProperty.call(value, "__rspyts_json__")
  ) {
    throw new TypeError("rspyts: expected an rspyts Json wrapper");
  }
  return (value as Record<string, unknown>)["__rspyts_json__"];
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

function exportFn(mod: BridgeModule, name: string): Function {
  const fn = mod.exports[name];
  if (typeof fn !== "function") {
    throw new Error(`rspyts: module has no export "${name}"`);
  }
  return fn;
}

function ensureHealthy(mod: BridgeModule): void {
  if (isPoisoned(mod)) {
    throw new InstancePoisonedError();
  }
}

function invokeWasm(mod: BridgeModule, fn: Function, args: Array<number | bigint>): unknown {
  try {
    return fn(...args);
  } catch (error) {
    if (error instanceof WebAssembly.RuntimeError) {
      markPoisoned(mod);
    }
    throw error;
  }
}

function allocRaw(mod: BridgeModule, len: number): number {
  const raw = invokeWasm(mod, exportFn(mod, "rspyts_alloc"), [len]);
  if (typeof raw !== "number") {
    throw new Error("rspyts: rspyts_alloc returned a non-numeric pointer");
  }
  return wasm32Pointer(raw, "rspyts_alloc");
}

function freeRaw(mod: BridgeModule, ptr: number, len: number): void {
  invokeWasm(mod, exportFn(mod, "rspyts_free"), [ptr, len]);
}

function wasm32Pointer(value: number, source: string): number {
  if (!Number.isInteger(value)) {
    throw new Error(`rspyts: ${source} returned a non-integer pointer`);
  }
  const ptr = value >>> 0;
  if (ptr === 0) {
    throw new Error(`rspyts: ${source} returned a null pointer`);
  }
  return ptr;
}

function ensureMemoryRange(mod: BridgeModule, ptr: number, len: number, label: string): void {
  const end = ptr + len;
  if (!Number.isSafeInteger(len) || len < 0 || end > mod.memory.buffer.byteLength) {
    throw new RangeError(
      `rspyts: ${label} range ${ptr}..${end} exceeds WebAssembly memory ` +
        `${mod.memory.buffer.byteLength}`,
    );
  }
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
 * Call a bridged export and return its decoded result.
 *
 * @param mod    The instantiated module.
 * @param symbol Export name, e.g. `"rspyts_fn__process_values"`.
 * @param args   Plain parameters as a wire-cased JSON object (`{}` when
 *               there are none — the pair is always passed, ABI §3.1).
 * @param slices `&[T]` parameters, in declaration order.
 * @param handle Leading `u64` handle for method calls. WASM `u64`
 *               parameters surface as JS `bigint`; pointers and lengths
 *               are `usize` (wasm32 `i32`) and cross as `number`s.
 * @returns The JSON payload with every `__rspyts_buf__` placeholder
 *          replaced by a typed array copied out of linear memory.
 * @throws A registered {@link RspytsError} subclass on status 1,
 *         {@link RspytsPanicError} on status 2.
 */
export function callFn(
  mod: BridgeModule,
  symbol: string,
  args: unknown,
  slices: SliceArg[] = [],
  handle?: bigint,
  errorTypes?: BridgeErrorRegistry,
): unknown {
  ensureHealthy(mod);
  const fn = exportFn(mod, symbol);
  const allocations: Allocation[] = [];
  let response: Allocation | undefined;
  let envelope: Uint8Array;
  try {
    const argsAlloc = writeBytes(mod, buildRequest(args ?? {}));
    allocations.push(argsAlloc);

    const callArgs: Array<number | bigint> = handle === undefined ? [] : [handle];
    callArgs.push(argsAlloc.ptr, argsAlloc.len);
    for (const slice of slices) {
      const sliceAlloc = writeSlice(mod, slice);
      allocations.push(sliceAlloc);
      callArgs.push(sliceAlloc.aligned, sliceAlloc.count);
    }

    const rawRetPtr = invokeWasm(mod, fn, callArgs);
    if (typeof rawRetPtr !== "number") {
      throw new Error(`rspyts: export "${symbol}" returned a non-pointer`);
    }
    const retPtr = wasm32Pointer(rawRetPtr, `export "${symbol}"`);

    // Fresh view: the call itself may have grown linear memory while
    // allocating the envelope.
    try {
      ensureMemoryRange(mod, retPtr, HEADER_LEN, "response header");
    } catch (error) {
      markPoisoned(mod);
      throw error;
    }
    const view = new DataView(mod.memory.buffer);
    const jsonLen = view.getUint32(retPtr + 4, true);
    const tailLen = view.getUint32(retPtr + 8, true);
    const total = HEADER_LEN + jsonLen + tailLen;
    response = { ptr: retPtr, len: total };
    try {
      ensureMemoryRange(mod, retPtr, total, "response envelope");
    } catch (error) {
      markPoisoned(mod);
      throw error;
    }
    // Copy the whole envelope out before freeing; nothing may reference
    // linear memory after this point.
    envelope = new Uint8Array(mod.memory.buffer, retPtr, total).slice();
  } finally {
    if (response !== undefined && !isPoisoned(mod)) {
      freeRaw(mod, response.ptr, response.len);
    }
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

  const { status, payload } = decodeEnvelope(envelope);
  if (status === STATUS_OK) {
    return payload;
  }
  throwBridgeError(status, payload, errorTypes);
}

/**
 * Invoke a `__drop` export (ABI §8). Fire-and-forget: `__drop` is
 * idempotent and infallible by contract, and this is called from
 * `free()`, `[Symbol.dispose]`, and `FinalizationRegistry` callbacks,
 * where a throw would be unactionable — so a trapped instance is
 * swallowed. A missing export (a codegen bug) still throws.
 */
export function callDrop(mod: BridgeModule, symbol: string, handle: bigint): void {
  if (isPoisoned(mod)) {
    return;
  }
  const fn = exportFn(mod, symbol);
  try {
    invokeWasm(mod, fn, [handle]);
  } catch {
    // Deliberately ignored; see doc comment.
  }
}
