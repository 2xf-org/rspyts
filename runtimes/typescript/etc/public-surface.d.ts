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
import { type BridgeErrorRegistry } from "./errors.js";
import { type BridgeModule } from "./module.js";
/**
 * One successful ABI-3 response position. `tail` is explicit and owned host
 * memory; generated schema decoders preserve it while selecting child values.
 */
export interface WireResponse<Value = unknown> {
    readonly value: Value;
    readonly tail: Uint8Array;
}
/** Create an explicit response context, primarily for attachment-free error data. */
export declare function wireResponse<Value>(value: Value, tail?: Uint8Array): WireResponse<Value>;
/** Convert a host bigint to the canonical signed 64-bit wire string. */
export declare function i64ToWire(value: bigint): string;
/** Convert a host bigint to the canonical unsigned 64-bit wire string. */
export declare function u64ToWire(value: bigint): string;
/** Validate a canonical signed 64-bit wire string and return a bigint. */
export declare function i64FromWire(response: WireResponse): bigint;
/** Validate a canonical unsigned 64-bit wire string and return a bigint. */
export declare function u64FromWire(response: WireResponse): bigint;
/** Validate a finite structured float and canonicalize signed zero. */
export declare function floatFromWire(response: WireResponse): number;
/** Validate a finite structured f32 without accepting values Rust cannot represent. */
export declare function f32FromWire(response: WireResponse): number;
/** Validate an exact JSON boolean. */
export declare function boolFromWire(response: WireResponse): boolean;
/** Validate an exact, bounded JSON integer. */
export declare function boundedIntFromWire(response: WireResponse, minimum: number, maximum: number): number;
/** Validate an exact JSON string. */
export declare function stringFromWire(response: WireResponse): string;
/** Validate a materialized opaque-bytes attachment. */
export declare function bytesFromWire(response: WireResponse): Uint8Array;
/** Validate a materialized numeric-buffer attachment and its declared dtype. */
export declare function bufferFromWire(response: WireResponse, dtype: Dtype): BufTypedArray;
/** Validate an exact JSON array. */
export declare function listFromWire(response: WireResponse): WireResponse[];
/** Validate an exact JSON object. */
export declare function mapFromWire(response: WireResponse): Record<string, WireResponse>;
/** Validate the exact field set of one generated struct or enum variant. */
export declare function objectFromWire(response: WireResponse, required: readonly string[], optional?: readonly string[]): Record<string, WireResponse>;
/** Validate the JSON-array representation of a fixed-length tuple. */
export declare function tupleFromWire(response: WireResponse, length: number): WireResponse[];
/** Validate one generated string-enum wire value. */
export declare function stringEnumFromWire<const Value extends string>(response: WireResponse, variants: readonly Value[]): Value;
/** Field requirements for one internally tagged data-enum variant. */
export interface WireVariantShape {
    readonly required: readonly string[];
    readonly optional?: readonly string[];
}
/** Validate and dispatch one internally tagged data-enum wire object. */
export declare function enumFromWire(response: WireResponse, tag: string, variants: Readonly<Record<string, WireVariantShape>>): Record<string, WireResponse>;
/** Validate the one-value null data type. */
export declare function nullFromWire(response: WireResponse): null;
/** Validate and copy a transparent value at a schema-declared `Json` position. */
export declare function jsonFromWire(response: WireResponse): unknown;
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
 * Invoke one ABI-3 generated export. This is the sole function-call path.
 * Successful calls return explicit `{ value, tail }` context for the
 * generated schema decoder; application errors and panics throw.
 */
export declare function callFn(mod: BridgeModule, symbol: string, args: unknown, slices?: SliceArg[], handle?: bigint, errorTypes?: BridgeErrorRegistry): WireResponse;
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
export declare const HEADER_LEN = 12;
/** Envelope status: successful return value. */
export declare const STATUS_OK = 0;
/** Envelope status: application error (the bridged function's `Err`). */
export declare const STATUS_ERROR = 1;
/** Envelope status: a panic was caught behind the shim. */
export declare const STATUS_PANIC = 2;
/** Wire names of the supported raw-buffer element types (ABI §6). */
export type Dtype = "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "u64" | "i64" | "f32" | "f64";
type AttachmentDtype = Dtype | "bytes";
/** The typed arrays that raw buffers and slices materialize as. */
export type BufTypedArray = Uint8Array | Int8Array | Uint16Array | Int16Array | Uint32Array | Int32Array | BigUint64Array | BigInt64Array | Float32Array | Float64Array;
interface DtypeInfo {
    readonly ctor: new (buffer: ArrayBuffer) => BufTypedArray;
    /** Element size in bytes; also the natural alignment of the element. */
    readonly bytes: number;
    readonly create: (length: number) => BufTypedArray;
    readonly read: (view: DataView, offset: number) => number | bigint;
    readonly write: (view: DataView, offset: number, value: number | bigint) => void;
}
/** Per-dtype typed-array constructor and element size. */
export declare const DTYPE: Record<Dtype, DtypeInfo>;
/**
 * The JSON key marking a tail-buffer placeholder. Mirrors
 * `BUF_PLACEHOLDER_KEY` in `rspyts-core`; never changes within an ABI
 * major version.
 */
export declare const BUF_PLACEHOLDER_KEY = "__rspyts_buf__";
declare const WIRE_BUFFER_MARKER: unique symbol;
/** Explicit marker for a numeric typed array nested in request JSON. */
export interface WireBuffer {
    readonly [WIRE_BUFFER_MARKER]: true;
    readonly data: BufTypedArray;
    readonly dt: Dtype;
}
/** Mark a typed array as a numeric `Buf<T>` request attachment. */
export declare function wireBuffer(data: BufTypedArray, dt: Dtype): WireBuffer;
/** Validate and copy a transparent schema-declared JSON request value. */
export declare function jsonToWire(value: unknown): unknown;
/** Encode JSON plus aligned attachments as a strict ABI-3 request. */
export declare function buildRequest(args: unknown): Uint8Array;
/** Validate and copy one transparent, schema-declared JSON value. */
export declare function checkedJsonValue(value: unknown): unknown;
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
export declare function decodeRequest(bytes: Uint8Array): DecodedRequest;
/**
 * Decode a complete ABI-3 response without interpreting schema-directed
 * attachment shapes. Error and panic responses must have an empty tail.
 */
export declare function decodeEnvelope(bytes: Uint8Array): DecodedEnvelope;
/** Materialize a buffer only at a generated schema-declared attachment position. */
export declare function bufferFromEnvelope(value: unknown, tail: Uint8Array, expected: AttachmentDtype): BufTypedArray;
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
 * - status 2 requires code `panic` and becomes a {@link RspytsPanicError};
 * - status 1 maps bridge-owned `staleHandle` directly, then consults the
 *   generated call-scoped registry, then falls back to {@link RspytsError}.
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
 * A prior WebAssembly runtime trap left this module instance unsafe to
 * call. Instantiate a fresh module before retrying.
 */
export declare class InstancePoisonedError extends Error {
    constructor();
}
/**
 * Constructor shape a registered error class must have: `code` is baked
 * into the subclass, so only `message` and `data` arrive at throw time.
 */
export type BridgeErrorConstructor = new (message: string, data?: unknown) => RspytsError;
export type BridgeErrorRegistry = Readonly<Record<string, BridgeErrorConstructor>>;
/**
 * Throw the error class corresponding to a non-ok envelope: panic status
 * beats the registry; otherwise the registry maps `code` to a generated
 * class, with {@link RspytsError} as the fallback.
 */
export declare function throwBridgeError(status: number, payload: unknown, errorTypes?: BridgeErrorRegistry): never;

// --- index.d.ts ---
/** Application-facing rspyts ABI-3 runtime surface. */
export { ABI_VERSION, ContractFingerprintMismatchError, instantiate, type BridgeModule, type InstantiateOptions, } from "./module.js";
export { InstancePoisonedError, RspytsError, RspytsPanicError, StaleHandleError, } from "./errors.js";

// --- internal/abi3.d.ts ---
/**
 * Versioned generator-facing ABI-3 surface.
 *
 * This subpath is emitted by `rspyts generate`; application code should use
 * generated clients and the package root instead.
 */
export { boolFromWire, boundedIntFromWire, bufferFromWire, bytesFromWire, callDrop, callFn, enumFromWire, f32FromWire, floatFromWire, i64FromWire, i64ToWire, jsonFromWire, listFromWire, mapFromWire, nullFromWire, objectFromWire, stringEnumFromWire, stringFromWire, tupleFromWire, u64FromWire, u64ToWire, wireResponse, type SliceArg, type WireResponse, type WireVariantShape, } from "../call.js";
export { jsonToWire, wireBuffer } from "../envelope.js";
export { RspytsError, type BridgeErrorRegistry, } from "../errors.js";
export { type BridgeModule, verifyModuleContract, } from "../module.js";

// --- module.d.ts ---
/** Loading, validating, and owning an ABI-3 bridged WebAssembly module. */
/** The ABI major version implemented by this runtime. */
export declare const ABI_VERSION = 3;
/** Options accepted by {@link instantiate}. */
export interface InstantiateOptions {
    /** Exact lowercase SHA-256 of the deterministic manifest JSON. */
    readonly expectedContractFingerprint?: string;
}
/**
 * An instantiated and ABI-validated rspyts module. The contract fingerprint
 * is loaded from the module once during instantiation and remains explicit.
 */
export interface BridgeModule {
    readonly exports: Record<string, Function> & {
        memory?: WebAssembly.Memory;
    };
    readonly memory: WebAssembly.Memory;
    readonly contractFingerprint: string;
}
/** Raised when generated bindings and the loaded binary describe different contracts. */
export declare class ContractFingerprintMismatchError extends Error {
    readonly expected: string;
    readonly actual: string;
    constructor(expected: string, actual: string);
}
/** @internal Whether a prior WebAssembly runtime trap poisoned this module. */
export declare function isPoisoned(mod: BridgeModule): boolean;
/** @internal Permanently poison a module after a WebAssembly runtime trap. */
export declare function markPoisoned(mod: BridgeModule): void;
/** @internal Reject calls after a WebAssembly trap. */
export declare function ensureHealthy(mod: BridgeModule): void;
/** @internal Resolve one required function export. */
export declare function exportFn(mod: BridgeModule, name: string): Function;
/** @internal Invoke WASM and poison the instance on runtime traps. */
export declare function invokeWasm(mod: BridgeModule, fn: Function, args: Array<number | bigint>): unknown;
/** @internal Convert a wasm32 return value into a non-null pointer. */
export declare function wasm32Pointer(value: number, source: string): number;
/** @internal Validate one range against the module's current memory buffer. */
export declare function ensureMemoryRange(mod: BridgeModule, ptr: number, len: number, label: string): void;
/** @internal Allocate exactly `len` bytes through the module allocator. */
export declare function allocRaw(mod: BridgeModule, len: number): number;
/** @internal Free one exact module-owned pointer/length pair. */
export declare function freeRaw(mod: BridgeModule, ptr: number, len: number): void;
/**
 * @internal Copy and free an envelope returned by a module export. The
 * response is freed with the exact header-derived total length. Header/range
 * violations poison the module because ownership can no longer be recovered
 * safely.
 */
export declare function takeOwnedEnvelope(mod: BridgeModule, symbol: string, rawPointer: unknown): Uint8Array;
/**
 * Compare a validated module with the fingerprint embedded in generated
 * bindings. Generated `createClient` functions call this before exposing any
 * bridged operation.
 */
export declare function verifyModuleContract(mod: BridgeModule, expected: string): void;
/**
 * Instantiate an ABI-3 module, validate all module-level exports, load its
 * deterministic contract fingerprint, and optionally compare it immediately.
 */
export declare function instantiate(source: WebAssembly.Module | BufferSource | Response | Promise<Response>, options?: InstantiateOptions): Promise<BridgeModule>;
