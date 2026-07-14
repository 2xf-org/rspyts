import { describe, expect, it } from "vitest";

import * as rspyts from "../src/index.js";
import { RspytsError } from "../src/errors.js";

describe("public surface", () => {
  it("exposes exactly the names generated code imports", () => {
    // codegen.md §5.1: generated modules import exactly these names and
    // nothing else. Type-only exports have no runtime presence.
    expect(Object.keys(rspyts).sort()).toEqual([
      "InstancePoisonedError",
      "RspytsError",
      "RspytsPanicError",
      "StaleHandleError",
      "callDrop",
      "callFn",
      "floatFromWire",
      "i64FromWire",
      "i64ToWire",
      "instantiate",
      "jsonFromWire",
      "registerError",
      "u64FromWire",
      "u64ToWire",
      "wireBuffer",
    ]);
  });

  it("re-exports the same bindings the submodules define", () => {
    expect(rspyts.RspytsError).toBe(RspytsError);
    expect(new rspyts.InstancePoisonedError()).toBeInstanceOf(Error);
    expect(new rspyts.RspytsPanicError("boom")).toBeInstanceOf(RspytsError);
    expect(rspyts.instantiate).toBeTypeOf("function");
    expect(rspyts.callFn).toBeTypeOf("function");
    expect(rspyts.callDrop).toBeTypeOf("function");
    expect(rspyts.floatFromWire).toBeTypeOf("function");
    expect(rspyts.i64FromWire).toBeTypeOf("function");
    expect(rspyts.i64ToWire).toBeTypeOf("function");
    expect(rspyts.jsonFromWire).toBeTypeOf("function");
    expect(rspyts.registerError).toBeTypeOf("function");
    expect(rspyts.wireBuffer).toBeTypeOf("function");
    expect(rspyts.u64FromWire).toBeTypeOf("function");
    expect(rspyts.u64ToWire).toBeTypeOf("function");
  });
});
