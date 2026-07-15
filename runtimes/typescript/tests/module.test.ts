import { describe, expect, it } from "vitest";

import {
  ABI_VERSION,
  ContractFingerprintMismatchError,
  instantiate,
  isPoisoned,
  verifyModuleContract,
} from "../src/module.js";

const FINGERPRINT = "0123456789abcdef".repeat(4);

function buildTinyModule(options: {
  abiVersion?: number;
  fingerprint?: string;
  exportMemory?: boolean;
  exportAbi?: boolean;
  exportAlloc?: boolean;
  exportFree?: boolean;
  exportFingerprint?: boolean;
  memoryAsFunction?: boolean;
} = {}) {
  const {
    abiVersion = ABI_VERSION,
    fingerprint = FINGERPRINT,
    exportMemory = true,
    exportAbi = true,
    exportAlloc = true,
    exportFree = true,
    exportFingerprint = true,
    memoryAsFunction = false,
  } = options;
  const envelopePointer = 1024;
  const json = new TextEncoder().encode(JSON.stringify(fingerprint));
  const envelope = new Uint8Array(12 + json.byteLength);
  new DataView(envelope.buffer).setUint32(4, json.byteLength, true);
  envelope.set(json, 12);

  const uleb = (value: number): number[] => {
    const out: number[] = [];
    let n = value >>> 0;
    do {
      let byte = n & 0x7f;
      n >>>= 7;
      if (n !== 0) byte |= 0x80;
      out.push(byte);
    } while (n !== 0);
    return out;
  };
  const section = (id: number, body: number[]): number[] => [id, ...uleb(body.length), ...body];
  const name = (value: string): number[] => {
    const bytes = Array.from(new TextEncoder().encode(value));
    return [...uleb(bytes.length), ...bytes];
  };
  const fnBody = (instructions: number[]): number[] => [
    ...uleb(instructions.length + 2),
    0x00,
    ...instructions,
    0x0b,
  ];

  const types = section(1, [
    ...uleb(3),
    0x60, 0x00, 0x01, 0x7f, // () -> i32
    0x60, 0x01, 0x7f, 0x01, 0x7f, // (i32) -> i32
    0x60, 0x02, 0x7f, 0x7f, 0x00, // (i32, i32) -> ()
  ]);
  const functions = section(3, [...uleb(4), 0, 0, 1, 2]);
  const memory = section(5, [...uleb(1), 0x00, 0x01]);
  const exports: number[][] = [];
  if (memoryAsFunction) exports.push([...name("memory"), 0x00, 0x00]);
  else if (exportMemory) exports.push([...name("memory"), 0x02, 0x00]);
  if (exportAbi) exports.push([...name("rspyts_abi_version"), 0x00, 0x00]);
  if (exportFingerprint) exports.push([...name("rspyts_contract_fingerprint"), 0x00, 0x01]);
  if (exportAlloc) exports.push([...name("rspyts_alloc"), 0x00, 0x02]);
  if (exportFree) exports.push([...name("rspyts_free"), 0x00, 0x03]);
  const exportSection = section(7, [...uleb(exports.length), ...exports.flat()]);
  const code = section(10, [
    ...uleb(4),
    ...fnBody([0x41, ...uleb(abiVersion)]),
    ...fnBody([0x41, ...uleb(envelopePointer)]),
    ...fnBody([0x41, ...uleb(2048)]),
    ...fnBody([]),
  ]);
  const data = section(11, [
    ...uleb(1),
    0x00,
    0x41,
    ...uleb(envelopePointer),
    0x0b,
    ...uleb(envelope.byteLength),
    ...envelope,
  ]);

  return new Uint8Array([
    0x00, 0x61, 0x73, 0x6d,
    0x01, 0x00, 0x00, 0x00,
    ...types,
    ...functions,
    ...memory,
    ...exportSection,
    ...code,
    ...data,
  ]);
}

const hasStreaming = typeof WebAssembly.instantiateStreaming === "function";
const wasmResponse = (bytes: Uint8Array<ArrayBuffer>) =>
  new Response(bytes, { headers: { "content-type": "application/wasm" } });

describe("instantiate", () => {
  it("loads ABI version and the explicit contract fingerprint", async () => {
    const mod = await instantiate(buildTinyModule(), {
      expectedContractFingerprint: FINGERPRINT,
    });
    expect(mod.memory).toBeInstanceOf(WebAssembly.Memory);
    expect(mod.contractFingerprint).toBe(FINGERPRINT);
    expect(isPoisoned(mod)).toBe(false);
  });

  it("accepts a WebAssembly.Module, ArrayBuffer, Response, and Promise<Response>", async () => {
    const bytes = buildTinyModule();
    expect((await instantiate(await WebAssembly.compile(bytes))).contractFingerprint).toBe(FINGERPRINT);
    expect((await instantiate(bytes.buffer)).contractFingerprint).toBe(FINGERPRINT);
    if (hasStreaming) {
      expect((await instantiate(wasmResponse(buildTinyModule()))).contractFingerprint).toBe(FINGERPRINT);
      expect(
        (await instantiate(Promise.resolve(wasmResponse(buildTinyModule())))).contractFingerprint,
      ).toBe(FINGERPRINT);
    }
  });

  it("rejects a different ABI before loading the contract", async () => {
    await expect(instantiate(buildTinyModule({ abiVersion: 2 }))).rejects.toThrow(
      /module reports 2, this runtime requires 3/,
    );
  });

  it("requires every ABI-3 module-level export", async () => {
    for (const [option, symbol] of [
      ["exportMemory", "memory"],
      ["exportAbi", "rspyts_abi_version"],
      ["exportAlloc", "rspyts_alloc"],
      ["exportFree", "rspyts_free"],
      ["exportFingerprint", "rspyts_contract_fingerprint"],
    ] as const) {
      await expect(instantiate(buildTinyModule({ [option]: false }))).rejects.toThrow(symbol);
    }
    await expect(instantiate(buildTinyModule({ memoryAsFunction: true }))).rejects.toThrow(
      /does not export "memory"/,
    );
  });

  it("validates module and expected fingerprint syntax", async () => {
    await expect(instantiate(buildTinyModule({ fingerprint: "ABC" }))).rejects.toThrow(
      /64 lowercase hexadecimal/,
    );
    await expect(
      instantiate(buildTinyModule(), { expectedContractFingerprint: `sha256:${FINGERPRINT}` }),
    ).rejects.toThrow(/64 lowercase hexadecimal/);
  });

  it("reports deterministic contract mismatch details", async () => {
    const expected = "f".repeat(64);
    await expect(
      instantiate(buildTinyModule(), { expectedContractFingerprint: expected }),
    ).rejects.toMatchObject({
      name: "ContractFingerprintMismatchError",
      expected,
      actual: FINGERPRINT,
    });
    await expect(
      instantiate(buildTinyModule(), { expectedContractFingerprint: expected }),
    ).rejects.toBeInstanceOf(ContractFingerprintMismatchError);
  });

  it("supports generated-client verification after application loading", async () => {
    const mod = await instantiate(buildTinyModule());
    expect(() => verifyModuleContract(mod, FINGERPRINT)).not.toThrow();
    expect(() => verifyModuleContract(mod, "f".repeat(64))).toThrow(
      ContractFingerprintMismatchError,
    );
  });
});
