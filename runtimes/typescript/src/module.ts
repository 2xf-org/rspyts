/** Loading, validating, and owning an ABI-3 bridged WebAssembly module. */

import { HEADER_LEN, STATUS_OK, decodeEnvelope } from "./envelope.js";
import { InstancePoisonedError } from "./errors.js";

/** The ABI major version implemented by this runtime. */
export const ABI_VERSION = 3;

const FINGERPRINT_EXPORT = "rspyts_contract_fingerprint";
const FINGERPRINT_PATTERN = /^[0-9a-f]{64}$/;

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
  readonly exports: Record<string, Function> & { memory?: WebAssembly.Memory };
  readonly memory: WebAssembly.Memory;
  readonly contractFingerprint: string;
}

/** Raised when generated bindings and the loaded binary describe different contracts. */
export class ContractFingerprintMismatchError extends Error {
  readonly expected: string;
  readonly actual: string;

  constructor(expected: string, actual: string) {
    super(
      `rspyts: contract fingerprint mismatch: generated client expects ${expected}, ` +
        `module reports ${actual} — regenerate the client from this exact module`,
    );
    this.name = "ContractFingerprintMismatchError";
    this.expected = expected;
    this.actual = actual;
  }
}

// Poisoning is lifecycle state, not response decoding context. Consumers
// cannot clear it and resume calls after a WebAssembly trap.
const poisonedModules = new WeakSet<BridgeModule>();

/** @internal Whether a prior WebAssembly runtime trap poisoned this module. */
export function isPoisoned(mod: BridgeModule): boolean {
  return poisonedModules.has(mod);
}

/** @internal Permanently poison a module after a WebAssembly runtime trap. */
export function markPoisoned(mod: BridgeModule): void {
  poisonedModules.add(mod);
}

/** @internal Reject calls after a WebAssembly trap. */
export function ensureHealthy(mod: BridgeModule): void {
  if (isPoisoned(mod)) throw new InstancePoisonedError();
}

/** @internal Resolve one required function export. */
export function exportFn(mod: BridgeModule, name: string): Function {
  const fn = mod.exports[name];
  if (typeof fn !== "function") {
    throw new Error(`rspyts: module has no export "${name}"`);
  }
  return fn;
}

/** @internal Invoke WASM and poison the instance on runtime traps. */
export function invokeWasm(
  mod: BridgeModule,
  fn: Function,
  args: Array<number | bigint>,
): unknown {
  try {
    return fn(...args);
  } catch (error) {
    if (error instanceof WebAssembly.RuntimeError) markPoisoned(mod);
    throw error;
  }
}

/** @internal Convert a wasm32 return value into a non-null pointer. */
export function wasm32Pointer(value: number, source: string): number {
  if (!Number.isInteger(value)) {
    throw new Error(`rspyts: ${source} returned a non-integer pointer`);
  }
  const ptr = value >>> 0;
  if (ptr === 0) throw new Error(`rspyts: ${source} returned a null pointer`);
  return ptr;
}

/** @internal Validate one range against the module's current memory buffer. */
export function ensureMemoryRange(
  mod: BridgeModule,
  ptr: number,
  len: number,
  label: string,
): void {
  const end = ptr + len;
  if (
    !Number.isSafeInteger(ptr) ||
    ptr < 0 ||
    !Number.isSafeInteger(len) ||
    len < 0 ||
    !Number.isSafeInteger(end) ||
    end > mod.memory.buffer.byteLength
  ) {
    throw new RangeError(
      `rspyts: ${label} range ${ptr}..${end} exceeds WebAssembly memory ` +
        `${mod.memory.buffer.byteLength}`,
    );
  }
}

/** @internal Allocate exactly `len` bytes through the module allocator. */
export function allocRaw(mod: BridgeModule, len: number): number {
  if (!Number.isInteger(len) || len < 0 || len > 0xffffffff) {
    throw new RangeError("rspyts: allocation length must fit unsigned wasm32 usize");
  }
  const raw = invokeWasm(mod, exportFn(mod, "rspyts_alloc"), [len]);
  if (typeof raw !== "number") {
    throw new Error("rspyts: rspyts_alloc returned a non-numeric pointer");
  }
  return wasm32Pointer(raw, "rspyts_alloc");
}

/** @internal Free one exact module-owned pointer/length pair. */
export function freeRaw(mod: BridgeModule, ptr: number, len: number): void {
  invokeWasm(mod, exportFn(mod, "rspyts_free"), [ptr, len]);
}

/**
 * @internal Copy and free an envelope returned by a module export. The
 * response is freed with the exact header-derived total length. Header/range
 * violations poison the module because ownership can no longer be recovered
 * safely.
 */
export function takeOwnedEnvelope(
  mod: BridgeModule,
  symbol: string,
  rawPointer: unknown,
): Uint8Array {
  if (typeof rawPointer !== "number") {
    throw new Error(`rspyts: export "${symbol}" returned a non-pointer`);
  }
  const ptr = wasm32Pointer(rawPointer, `export "${symbol}"`);
  let total: number | undefined;
  try {
    try {
      ensureMemoryRange(mod, ptr, HEADER_LEN, "response header");
      const header = new DataView(mod.memory.buffer, ptr, HEADER_LEN);
      const jsonLen = header.getUint32(4, true);
      const tailLen = header.getUint32(8, true);
      total = HEADER_LEN + jsonLen + tailLen;
      ensureMemoryRange(mod, ptr, total, "response envelope");
    } catch (error) {
      markPoisoned(mod);
      throw error;
    }
    return new Uint8Array(mod.memory.buffer, ptr, total).slice();
  } finally {
    if (total !== undefined && !isPoisoned(mod)) {
      freeRaw(mod, ptr, total);
    }
  }
}

/**
 * Compare a validated module with the fingerprint embedded in generated
 * bindings. Generated `createClient` functions call this before exposing any
 * bridged operation.
 */
export function verifyModuleContract(mod: BridgeModule, expected: string): void {
  validateFingerprint(expected, "expected contract fingerprint");
  if (mod.contractFingerprint !== expected) {
    throw new ContractFingerprintMismatchError(expected, mod.contractFingerprint);
  }
}

/**
 * Instantiate an ABI-3 module, validate all module-level exports, load its
 * deterministic contract fingerprint, and optionally compare it immediately.
 */
export async function instantiate(
  source: WebAssembly.Module | BufferSource | Response | Promise<Response>,
  options: InstantiateOptions = {},
): Promise<BridgeModule> {
  const instance = await instantiateAny(source);
  const raw = instance.exports as Record<string, unknown>;
  const memory = raw["memory"];
  if (!(memory instanceof WebAssembly.Memory)) {
    throw new Error(
      'rspyts: module does not export "memory" — build the bridged crate as a wasm32 cdylib',
    );
  }

  for (const name of ["rspyts_abi_version", "rspyts_alloc", "rspyts_free", FINGERPRINT_EXPORT]) {
    if (typeof raw[name] !== "function") {
      throw new Error(
        `rspyts: module does not export "${name}" — is this an ABI-3 rspyts module ` +
          "(does the crate invoke rspyts::export!())?",
      );
    }
  }

  const reported: unknown = (raw["rspyts_abi_version"] as Function)();
  if (reported !== ABI_VERSION) {
    throw new Error(
      `rspyts: ABI version mismatch: module reports ${String(reported)}, ` +
        `this runtime requires ${ABI_VERSION} — regenerate with a matching rspyts CLI ` +
        "or upgrade the npm package",
    );
  }

  const provisional: BridgeModule = {
    exports: instance.exports as BridgeModule["exports"],
    memory,
    contractFingerprint: "",
  };
  const fingerprint = loadContractFingerprint(provisional);
  const mod: BridgeModule = { ...provisional, contractFingerprint: fingerprint };
  if (options.expectedContractFingerprint !== undefined) {
    verifyModuleContract(mod, options.expectedContractFingerprint);
  }
  return mod;
}

function loadContractFingerprint(mod: BridgeModule): string {
  const symbol = FINGERPRINT_EXPORT;
  const rawPointer = invokeWasm(mod, exportFn(mod, symbol), []);
  const envelope = takeOwnedEnvelope(mod, symbol, rawPointer);
  const { status, value, tail } = decodeEnvelope(envelope);
  if (status !== STATUS_OK) {
    throw new Error(`rspyts: ${symbol} returned response status ${status}; expected success`);
  }
  if (tail.byteLength !== 0) {
    throw new Error(`rspyts: ${symbol} response must not contain attachment bytes`);
  }
  validateFingerprint(value, "module contract fingerprint");
  return value;
}

function validateFingerprint(value: unknown, label: string): asserts value is string {
  if (typeof value !== "string" || !FINGERPRINT_PATTERN.test(value)) {
    throw new TypeError(`rspyts: ${label} must be exactly 64 lowercase hexadecimal characters`);
  }
}

async function instantiateAny(
  source: WebAssembly.Module | BufferSource | Response | Promise<Response>,
): Promise<WebAssembly.Instance> {
  const imports: WebAssembly.Imports = {};
  if (source instanceof WebAssembly.Module) {
    return WebAssembly.instantiate(source, imports);
  }
  if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) {
    const { instance } = await WebAssembly.instantiate(source, imports);
    return instance;
  }
  const { instance } = await WebAssembly.instantiateStreaming(source, imports);
  return instance;
}
