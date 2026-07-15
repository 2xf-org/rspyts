import { describe, expect, it } from "vitest";

import * as internal from "../src/internal/abi3.js";
import * as rspyts from "../src/index.js";
import { RspytsError } from "../src/errors.js";

describe("package surfaces", () => {
  it("keeps the package root application-facing and small", () => {
    expect(Object.keys(rspyts).sort()).toEqual([
      "ABI_VERSION",
      "ContractFingerprintMismatchError",
      "InstancePoisonedError",
      "RspytsError",
      "RspytsPanicError",
      "StaleHandleError",
      "instantiate",
    ]);
    expect(rspyts.ABI_VERSION).toBe(3);
    expect(rspyts.RspytsError).toBe(RspytsError);
    expect(rspyts.instantiate).toBeTypeOf("function");
  });

  it("exposes the exact ABI-3 emitter runtime from the versioned internal subpath", () => {
    expect(Object.keys(internal).sort()).toEqual([
      "RspytsError",
      "boolFromWire",
      "boundedIntFromWire",
      "bufferFromWire",
      "bytesFromWire",
      "callDrop",
      "callFn",
      "enumFromWire",
      "f32FromWire",
      "floatFromWire",
      "i64FromWire",
      "i64ToWire",
      "jsonFromWire",
      "jsonToWire",
      "listFromWire",
      "mapFromWire",
      "nullFromWire",
      "objectFromWire",
      "stringEnumFromWire",
      "stringFromWire",
      "tupleFromWire",
      "u64FromWire",
      "u64ToWire",
      "verifyModuleContract",
      "wireBuffer",
      "wireResponse",
    ]);
    expect(internal.callFn).toBeTypeOf("function");
    expect(internal.verifyModuleContract).toBeTypeOf("function");
  });
});
