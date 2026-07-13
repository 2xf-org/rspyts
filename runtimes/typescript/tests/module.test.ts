import { describe, expect, it } from "vitest";

import { instantiate } from "../src/module.js";

/**
 * Hand-assembled minimal wasm module: one exported memory (1 page) and a
 * `rspyts_abi_version() -> i32` function returning a configurable
 * constant. Small enough to build byte-by-byte; real enough to exercise
 * every `instantiate` code path.
 */
// Return type deliberately inferred: `new Uint8Array([...])` is
// `Uint8Array<ArrayBuffer>` under TS ≥ 5.7 libs, which `BufferSource` and
// `BodyInit` require.
function buildTinyModule(options: {
  abiVersion?: number;
  exportMemory?: boolean;
  exportAbi?: boolean;
  /** Export the abi function itself under the name "memory" instead of
   * the real memory — a "memory" export that is not a WebAssembly.Memory. */
  memoryAsFunction?: boolean;
} = {}) {
  const { abiVersion = 1, exportMemory = true, exportAbi = true, memoryAsFunction = false } = options;

  const uleb = (value: number): number[] => {
    const out: number[] = [];
    let n = value;
    do {
      let byte = n & 0x7f;
      n >>>= 7;
      if (n !== 0) {
        byte |= 0x80;
      }
      out.push(byte);
    } while (n !== 0);
    return out;
  };
  const section = (id: number, body: number[]): number[] => [id, ...uleb(body.length), ...body];
  const name = (s: string): number[] => {
    const bytes = Array.from(new TextEncoder().encode(s));
    return [...uleb(bytes.length), ...bytes];
  };

  const typeSection = section(1, [...uleb(1), 0x60, 0x00, 0x01, 0x7f]); // () -> i32
  const funcSection = section(3, [...uleb(1), 0x00]);
  const memSection = section(5, [...uleb(1), 0x00, 0x01]); // min 1 page
  const exports: number[][] = [];
  if (memoryAsFunction) {
    exports.push([...name("memory"), 0x00, 0x00]);
  } else if (exportMemory) {
    exports.push([...name("memory"), 0x02, 0x00]);
  }
  if (exportAbi) {
    exports.push([...name("rspyts_abi_version"), 0x00, 0x00]);
  }
  const exportSection = section(7, [...uleb(exports.length), ...exports.flat()]);
  const body = [0x00, 0x41, ...uleb(abiVersion), 0x0b]; // no locals; i32.const; end
  const codeSection = section(10, [...uleb(1), ...uleb(body.length), ...body]);

  return new Uint8Array([
    0x00, 0x61, 0x73, 0x6d, // magic
    0x01, 0x00, 0x00, 0x00, // version
    ...typeSection,
    ...funcSection,
    ...memSection,
    ...exportSection,
    ...codeSection,
  ]);
}

const hasStreaming = typeof WebAssembly.instantiateStreaming === "function";

function wasmResponse(bytes: ReturnType<typeof buildTinyModule>): Response {
  return new Response(bytes, { headers: { "content-type": "application/wasm" } });
}

describe("instantiate", () => {
  it("accepts compiled bytes (BufferSource)", async () => {
    const mod = await instantiate(buildTinyModule());
    expect(mod.memory).toBeInstanceOf(WebAssembly.Memory);
    expect(mod.exports["rspyts_abi_version"]).toBeTypeOf("function");
    expect(mod.memory.buffer.byteLength).toBe(65536);
  });

  it("accepts a precompiled WebAssembly.Module", async () => {
    const module = await WebAssembly.compile(buildTinyModule());
    const mod = await instantiate(module);
    expect(mod.memory).toBeInstanceOf(WebAssembly.Memory);
  });

  it("accepts a bare ArrayBuffer (the other BufferSource shape)", async () => {
    const bytes = buildTinyModule();
    const mod = await instantiate(bytes.buffer);
    expect(mod.memory).toBeInstanceOf(WebAssembly.Memory);
    expect(mod.exports["rspyts_abi_version"]).toBeTypeOf("function");
  });

  it.runIf(hasStreaming)("accepts a Response via instantiateStreaming", async () => {
    const mod = await instantiate(wasmResponse(buildTinyModule()));
    expect(mod.memory).toBeInstanceOf(WebAssembly.Memory);
  });

  it.runIf(hasStreaming)("accepts a Promise<Response>", async () => {
    const mod = await instantiate(Promise.resolve(wasmResponse(buildTinyModule())));
    expect(mod.memory).toBeInstanceOf(WebAssembly.Memory);
  });

  it("rejects a module reporting a different ABI version", async () => {
    await expect(instantiate(buildTinyModule({ abiVersion: 2 }))).rejects.toThrow(
      /ABI version mismatch: module reports 2/,
    );
  });

  it("names both versions and a remedy in the ABI mismatch error", async () => {
    await expect(instantiate(buildTinyModule({ abiVersion: 0 }))).rejects.toThrow(
      /module reports 0, this runtime requires 1 — regenerate/,
    );
  });

  it("rejects a module that does not export memory", async () => {
    await expect(instantiate(buildTinyModule({ exportMemory: false }))).rejects.toThrow(
      /does not export "memory"/,
    );
  });

  it('rejects a "memory" export that is not a WebAssembly.Memory', async () => {
    await expect(instantiate(buildTinyModule({ memoryAsFunction: true }))).rejects.toThrow(
      /does not export "memory"/,
    );
  });

  it("rejects a module that does not export rspyts_abi_version", async () => {
    await expect(instantiate(buildTinyModule({ exportAbi: false }))).rejects.toThrow(
      /does not export "rspyts_abi_version"/,
    );
  });
});
