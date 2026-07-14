// --- call.d.ts ---
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
import { type BufTypedArray, type Dtype } from "./envelope.js";
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
export declare function writeBytes(mod: BridgeModule, bytes: Uint8Array): Allocation;
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
export declare function writeSlice(mod: BridgeModule, slice: SliceArg): SliceAllocation;
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
export declare function callFn(mod: BridgeModule, symbol: string, args: unknown, slices?: SliceArg[], handle?: bigint): unknown;
/**
 * Invoke a `__drop` export (ABI §8). Fire-and-forget: `__drop` is
 * idempotent and infallible by contract, and this is called from
 * `free()`, `[Symbol.dispose]`, and `FinalizationRegistry` callbacks,
 * where a throw would be unactionable — so a trapped instance is
 * swallowed. A missing export (a codegen bug) still throws.
 */
export declare function callDrop(mod: BridgeModule, symbol: string, handle: bigint): void;
export {};

// --- envelope.d.ts ---
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
export declare const HEADER_LEN = 12;
/** Envelope status: successful return value. */
export declare const STATUS_OK = 0;
/** Envelope status: application error (the bridged function's `Err`). */
export declare const STATUS_ERROR = 1;
/** Envelope status: a panic was caught behind the shim. */
export declare const STATUS_PANIC = 2;
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
export declare const DTYPE: Record<Dtype, DtypeInfo>;
/**
 * The JSON key marking a tail-buffer placeholder. Mirrors
 * `BUF_PLACEHOLDER_KEY` in `rspyts-core`; never changes within an ABI
 * major version.
 */
export declare const BUF_PLACEHOLDER_KEY = "__rspyts_buf__";
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
export declare function decodeEnvelope(bytes: Uint8Array): DecodedEnvelope;
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
export declare function substituteBuffers(value: unknown, tail: Uint8Array): unknown;
export {};

// --- errors.d.ts ---
/**
 * The rspyts error surface (ABI §5).
 *
 * A failed bridged call crosses the boundary as a JSON object
 * `{ code, message, data? }` inside a status-1 (application error) or
 * status-2 (panic) envelope. This module maps that object back onto a
 * JavaScript error class:
 *
 * - status 2 always becomes a {@link RspytsPanicError}, regardless of code;
 * - status 1 consults the code registry populated at import time by
 *   generated `errors.ts` modules (via {@link registerError}), falling back
 *   to the {@link RspytsError} base class for unregistered codes.
 */
/**
 * Base class for every error crossing the rspyts bridge.
 *
 * Generated error classes extend this and bake in their `code`; `message`
 * is the human-readable text from Rust and `data` carries the error
 * variant's named fields (or `undefined`).
 */
export declare class RspytsError extends Error {
    code: string;
    data: unknown;
    constructor(message: string, code: string, data?: unknown);
}
/** A Rust panic was caught behind the shim (envelope status 2, ABI §9). */
export declare class RspytsPanicError extends RspytsError {
    constructor(message: string, data?: unknown);
}
/** A method was called on a dropped or unknown handle (ABI §8). */
export declare class StaleHandleError extends RspytsError {
    constructor(message: string, data?: unknown);
}
/**
 * Constructor shape a registered error class must have: `code` is baked
 * into the subclass, so only `message` and `data` arrive at throw time.
 */
export type BridgeErrorConstructor = new (message: string, data?: unknown) => RspytsError;
/**
 * Register an error class for a bridge error `code`. Generated `errors.ts`
 * modules call this at import time; a later registration for the same code
 * replaces the earlier one.
 */
export declare function registerError(code: string, ctor: BridgeErrorConstructor): void;
/**
 * Throw the error class corresponding to a non-ok envelope: panic status
 * beats the registry; otherwise the registry maps `code` to a generated
 * class, with {@link RspytsError} as the fallback.
 */
export declare function throwBridgeError(status: number, payload: unknown): never;

// --- index.d.ts ---
/**
 * rspyts TypeScript runtime — the support library for code generated by
 * `rspyts generate` (see `docs/design/codegen.md` §5.1 in the repository).
 *
 * This is the complete public surface; generated code imports exactly
 * these names and nothing else.
 */
export { instantiate, type BridgeModule } from "./module.js";
export { callFn, callDrop, type SliceArg } from "./call.js";
export { RspytsError, RspytsPanicError, StaleHandleError, registerError } from "./errors.js";

// --- module.d.ts ---
/**
 * Loading a bridged WebAssembly module (ABI §3).
 *
 * rspyts modules are self-contained `wasm32-unknown-unknown` cdylibs: they
 * import nothing and export linear memory plus the `rspyts_*` symbols.
 * {@link instantiate} accepts anything the WebAssembly JS API can
 * instantiate, verifies the module speaks ABI version 1, and returns the
 * {@link BridgeModule} that `callFn`/`callDrop` operate on.
 */
/** The ABI major version this runtime implements (ABI §3). */
export declare const ABI_VERSION = 1;
/**
 * An instantiated rspyts module: the raw export object plus its linear
 * memory. `memory` is also reachable as `exports.memory`; it is hoisted
 * here so callers never touch a possibly-absent property.
 */
export interface BridgeModule {
    exports: Record<string, Function> & {
        memory?: WebAssembly.Memory;
    };
    memory: WebAssembly.Memory;
}
/**
 * Instantiate a bridged module from compiled bytes, a precompiled
 * `WebAssembly.Module`, or a (promised) HTTP `Response` — the latter two
 * via `WebAssembly.instantiateStreaming`, so `instantiate(fetch(url))`
 * compiles while downloading.
 *
 * Throws if the module does not export linear memory, does not export
 * `rspyts_abi_version`, or reports an ABI version other than
 * {@link ABI_VERSION}.
 */
export declare function instantiate(source: WebAssembly.Module | BufferSource | Response | Promise<Response>): Promise<BridgeModule>;
