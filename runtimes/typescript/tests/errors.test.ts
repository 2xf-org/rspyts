import { describe, expect, it } from "vitest";

import {
  InstancePoisonedError,
  RspytsError,
  RspytsPanicError,
  StaleHandleError,
  throwBridgeError,
} from "../src/errors.js";

function capture(fn: () => never): unknown {
  try {
    fn();
  } catch (error) {
    return error;
  }
}

describe("error classes", () => {
  it("preserves message, code, data, names, and instanceof chains", () => {
    const base = new RspytsError("it broke", "somethingBroke", { at: 3 });
    expect(base).toMatchObject({
      name: "RspytsError",
      message: "it broke",
      code: "somethingBroke",
      data: { at: 3 },
    });
    const panic = new RspytsPanicError("boom");
    expect(panic).toBeInstanceOf(RspytsError);
    expect(panic.code).toBe("panic");
    const stale = new StaleHandleError("gone");
    expect(stale).toBeInstanceOf(RspytsError);
    expect(stale.code).toBe("staleHandle");
  });

  it("keeps poisoned-instance failure separate and actionable", () => {
    const error = new InstancePoisonedError();
    expect(error).toBeInstanceOf(Error);
    expect(error).not.toBeInstanceOf(RspytsError);
    expect(error.name).toBe("InstancePoisonedError");
    expect(error.message).toMatch(/instantiate a fresh module/);
  });
});

describe("throwBridgeError", () => {
  it("uses panic status before any code or scoped class", () => {
    class Hijack extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "panic", data);
      }
    }
    const error = capture(() =>
      throwBridgeError(2, { code: "panic", message: "kaboom", data: 1 }, { panic: Hijack }),
    );
    expect(error).toBeInstanceOf(RspytsPanicError);
    expect(error).not.toBeInstanceOf(Hijack);
    expect(error).toMatchObject({ message: "kaboom", data: 1 });
  });

  it("maps bridge-owned stale handles without a process-global registry", () => {
    const error = capture(() =>
      throwBridgeError(1, { code: "staleHandle", message: "gone" }),
    );
    expect(error).toBeInstanceOf(StaleHandleError);
  });

  it("uses only the current call-scoped error map", () => {
    class First extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "shared", data);
      }
    }
    class Second extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "shared", data);
      }
    }
    const payload = { code: "shared", message: "scoped", data: { value: 1 } };
    expect(capture(() => throwBridgeError(1, payload, { shared: First }))).toBeInstanceOf(First);
    expect(capture(() => throwBridgeError(1, payload, { shared: Second }))).toBeInstanceOf(Second);
    const fallback = capture(() => throwBridgeError(1, payload));
    expect(fallback).toBeInstanceOf(RspytsError);
    expect(fallback).not.toBeInstanceOf(First);
    expect(fallback).toMatchObject({ code: "shared", data: { value: 1 } });
  });

  it("ignores inherited scoped constructors", () => {
    const inherited = Object.create({ evil: StaleHandleError }) as Record<
      string,
      typeof StaleHandleError
    >;
    const error = capture(() =>
      throwBridgeError(1, { code: "evil", message: "unregistered" }, inherited),
    );
    expect(error).toBeInstanceOf(RspytsError);
    expect(error).not.toBeInstanceOf(StaleHandleError);
  });

  it("rejects invalid own scoped registry entries clearly", () => {
    const invalid = { broken: 42 } as unknown as Parameters<typeof throwBridgeError>[2];
    expect(() =>
      throwBridgeError(1, { code: "broken", message: "bad" }, invalid),
    ).toThrow(/must be an error constructor/);
    class NotRspytsError extends Error {}
    const wrongBase = {
      broken: NotRspytsError,
    } as unknown as Parameters<typeof throwBridgeError>[2];
    expect(() =>
      throwBridgeError(1, { code: "broken", message: "bad" }, wrongBase),
    ).toThrow(/must construct RspytsError/);
  });

  it("strictly rejects malformed error payloads and panic codes", () => {
    expect(() => throwBridgeError(0, { code: "x", message: "bad" })).toThrow(
      /status must be 1 or 2/,
    );
    expect(() => throwBridgeError(3, { code: "x", message: "bad" })).toThrow(
      /status must be 1 or 2/,
    );
    for (const payload of [
      null,
      [],
      "bad",
      {},
      { code: "", message: "bad" },
      { code: 1, message: "bad" },
      { code: "x", message: 1 },
      { code: "x", message: "bad", extra: true },
    ]) {
      expect(() => throwBridgeError(1, payload), JSON.stringify(payload)).toThrow(TypeError);
    }
    expect(() => throwBridgeError(2, { code: "notPanic", message: "bad" })).toThrow(
      /must use error code "panic"/,
    );
  });
});
