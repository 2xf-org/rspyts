/**
 * Loading a bridged WebAssembly module (ABI §3).
 *
 * rspyts modules are self-contained `wasm32-unknown-unknown` cdylibs: they
 * import nothing and export linear memory plus the `rspyts_*` symbols.
 * {@link instantiate} accepts anything the WebAssembly JS API can
 * instantiate, verifies the module speaks ABI version 2, and returns the
 * {@link BridgeModule} that `callFn`/`callDrop` operate on.
 */

/** The ABI major version this runtime implements (ABI §3). */
export const ABI_VERSION = 2;

/**
 * An instantiated rspyts module: the raw export object plus its linear
 * memory. `memory` is also reachable as `exports.memory`; it is hoisted
 * here so callers never touch a possibly-absent property. A WebAssembly
 * runtime trap poisons the instance permanently because its Rust-side
 * invariants and allocator state can no longer be trusted.
 */
export interface BridgeModule {
  exports: Record<string, Function> & { memory?: WebAssembly.Memory };
  memory: WebAssembly.Memory;
}

// Kept outside the public object so consumers cannot accidentally clear a
// poison flag and resume calls into an unsafe instance.
const poisonedModules = new WeakSet<BridgeModule>();

/** @internal Whether a prior WebAssembly runtime trap poisoned this module. */
export function isPoisoned(mod: BridgeModule): boolean {
  return poisonedModules.has(mod);
}

/** @internal Permanently poison a module after a WebAssembly runtime trap. */
export function markPoisoned(mod: BridgeModule): void {
  poisonedModules.add(mod);
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
export async function instantiate(
  source: WebAssembly.Module | BufferSource | Response | Promise<Response>,
): Promise<BridgeModule> {
  const instance = await instantiateAny(source);
  const raw = instance.exports as Record<string, unknown>;

  const memory = raw["memory"];
  if (!(memory instanceof WebAssembly.Memory)) {
    throw new Error(
      'rspyts: module does not export "memory" — build the bridged crate as a wasm32 cdylib',
    );
  }
  const abiVersion = raw["rspyts_abi_version"];
  if (typeof abiVersion !== "function") {
    throw new Error(
      'rspyts: module does not export "rspyts_abi_version" — is this an rspyts-bridged module ' +
        "(does the crate invoke rspyts::export!())?",
    );
  }
  const reported: unknown = abiVersion();
  if (reported !== ABI_VERSION) {
    throw new Error(
      `rspyts: ABI version mismatch: module reports ${String(reported)}, ` +
        `this runtime requires ${ABI_VERSION} — regenerate with a matching rspyts CLI ` +
        "or upgrade the npm package",
    );
  }

  return { exports: instance.exports as BridgeModule["exports"], memory };
}

async function instantiateAny(
  source: WebAssembly.Module | BufferSource | Response | Promise<Response>,
): Promise<WebAssembly.Instance> {
  // rspyts modules import nothing (ABI §1: no per-language native
  // integration); an empty import object is part of the contract.
  const imports: WebAssembly.Imports = {};
  if (source instanceof WebAssembly.Module) {
    return WebAssembly.instantiate(source, imports);
  }
  if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) {
    const { instance } = await WebAssembly.instantiate(source, imports);
    return instance;
  }
  // Response | Promise<Response>: instantiateStreaming accepts both.
  const { instance } = await WebAssembly.instantiateStreaming(source, imports);
  return instance;
}
