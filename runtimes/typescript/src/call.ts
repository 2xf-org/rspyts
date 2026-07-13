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
  decodeEnvelope,
  type BufTypedArray,
  type Dtype,
} from "./envelope.js";
import { throwBridgeError } from "./errors.js";
import type { BridgeModule } from "./module.js";

/**
 * One `&[T]` slice argument: the data to pass and its wire dtype. The
 * typed array's constructor must match `dt` (`Float64Array` for `"f64"`,
 * etc.); plain arrays are deliberately not accepted.
 */
export interface SliceArg {
  data: BufTypedArray;
  dt: Dtype;
}

const utf8Encoder = new TextEncoder();

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

function allocRaw(mod: BridgeModule, len: number): number {
  const ptr: unknown = exportFn(mod, "rspyts_alloc")(len);
  if (typeof ptr !== "number") {
    throw new Error("rspyts: rspyts_alloc returned a non-numeric pointer");
  }
  return ptr;
}

function freeRaw(mod: BridgeModule, ptr: number, len: number): void {
  exportFn(mod, "rspyts_free")(ptr, len);
}

/**
 * Copy `bytes` into a fresh `rspyts_alloc` allocation. The memory view is
 * created only after the allocation returns: growing linear memory
 * detaches every earlier view.
 */
export function writeBytes(mod: BridgeModule, bytes: Uint8Array): Allocation {
  const len = bytes.byteLength;
  const ptr = allocRaw(mod, len);
  if (len > 0) {
    new Uint8Array(mod.memory.buffer, ptr, len).set(bytes);
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
  // Build the source view BEFORE allocating: constructing a view over a
  // detached buffer (e.g. transferred via postMessage) throws, and doing
  // so after rspyts_alloc would leak the allocation.
  const src = new Uint8Array(slice.data.buffer, slice.data.byteOffset, byteLen);
  const align = info.bytes;
  const len = byteLen + align;
  const ptr = allocRaw(mod, len);
  const aligned = ptr + ((align - (ptr % align)) % align);
  new Uint8Array(mod.memory.buffer, aligned, byteLen).set(src);
  return { ptr, len, aligned, count: slice.data.length };
}

/**
 * Call a bridged export and return its decoded result.
 *
 * @param mod    The instantiated module.
 * @param symbol Export name, e.g. `"rspyts_fn__analyze_signal"`.
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
): unknown {
  const fn = exportFn(mod, symbol);
  const allocations: Allocation[] = [];
  let envelope: Uint8Array;
  try {
    const argsAlloc = writeBytes(mod, utf8Encoder.encode(JSON.stringify(args ?? {})));
    allocations.push(argsAlloc);

    const callArgs: Array<number | bigint> = handle === undefined ? [] : [handle];
    callArgs.push(argsAlloc.ptr, argsAlloc.len);
    for (const slice of slices) {
      const sliceAlloc = writeSlice(mod, slice);
      allocations.push(sliceAlloc);
      callArgs.push(sliceAlloc.aligned, sliceAlloc.count);
    }

    const retPtr: unknown = fn(...callArgs);
    if (typeof retPtr !== "number") {
      throw new Error(`rspyts: export "${symbol}" returned a non-pointer`);
    }

    // Fresh view: the call itself may have grown linear memory while
    // allocating the envelope.
    const view = new DataView(mod.memory.buffer);
    const jsonLen = view.getUint32(retPtr + 4, true);
    const tailLen = view.getUint32(retPtr + 8, true);
    const total = HEADER_LEN + jsonLen + tailLen;
    // Copy the whole envelope out before freeing; nothing may reference
    // linear memory after this point.
    envelope = new Uint8Array(mod.memory.buffer, retPtr, total).slice();
    freeRaw(mod, retPtr, total);
  } finally {
    // Request buffers stay alive for the duration of the call (the callee
    // borrows, never frees — ABI §2) and are released even when the call
    // traps.
    for (const allocation of allocations) {
      freeRaw(mod, allocation.ptr, allocation.len);
    }
  }

  const { status, payload } = decodeEnvelope(envelope);
  if (status === STATUS_OK) {
    return payload;
  }
  throwBridgeError(status, payload);
}

/**
 * Invoke a `__drop` export (ABI §8). Fire-and-forget: `__drop` is
 * idempotent and infallible by contract, and this is called from
 * `free()`, `[Symbol.dispose]`, and `FinalizationRegistry` callbacks,
 * where a throw would be unactionable — so a trapped instance is
 * swallowed. A missing export (a codegen bug) still throws.
 */
export function callDrop(mod: BridgeModule, symbol: string, handle: bigint): void {
  const fn = exportFn(mod, symbol);
  try {
    fn(handle);
  } catch {
    // Deliberately ignored; see doc comment.
  }
}
