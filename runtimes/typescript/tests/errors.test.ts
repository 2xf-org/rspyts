import { describe, expect, it } from "vitest";

import {
  RspytsError,
  RspytsPanicError,
  StaleHandleError,
  registerError,
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
  it("carries message, code, and data", () => {
    const error = new RspytsError("it broke", "somethingBroke", { at: 3 });
    expect(error).toBeInstanceOf(Error);
    expect(error.message).toBe("it broke");
    expect(error.code).toBe("somethingBroke");
    expect(error.data).toEqual({ at: 3 });
    expect(error.name).toBe("RspytsError");
  });

  it("names subclasses via new.target", () => {
    expect(new RspytsPanicError("boom").name).toBe("RspytsPanicError");
    expect(new StaleHandleError("gone").name).toBe("StaleHandleError");
    class CustomNameError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "customName", data);
      }
    }
    expect(new CustomNameError("x").name).toBe("CustomNameError");
  });

  it("bakes fixed codes into the built-in subclasses", () => {
    expect(new RspytsPanicError("boom").code).toBe("panic");
    expect(new StaleHandleError("gone").code).toBe("staleHandle");
  });

  it("keeps the full instanceof chain intact for every built-in class", () => {
    const panic = new RspytsPanicError("boom");
    expect(panic).toBeInstanceOf(RspytsPanicError);
    expect(panic).toBeInstanceOf(RspytsError);
    expect(panic).toBeInstanceOf(Error);

    const stale = new StaleHandleError("gone");
    expect(stale).toBeInstanceOf(StaleHandleError);
    expect(stale).toBeInstanceOf(RspytsError);
    expect(stale).toBeInstanceOf(Error);
    expect(stale).not.toBeInstanceOf(RspytsPanicError);

    const base = new RspytsError("m", "c");
    expect(base).toBeInstanceOf(Error);
    expect(base).not.toBeInstanceOf(RspytsPanicError);
    expect(base).not.toBeInstanceOf(StaleHandleError);
  });
});

describe("throwBridgeError", () => {
  it("throws RspytsPanicError for status 2 regardless of code or registry", () => {
    class HijackPanicError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "panic", data);
      }
    }
    registerError("panic", HijackPanicError);
    const error = capture(() =>
      throwBridgeError(2, { code: "panic", message: "kaboom 7" }),
    );
    expect(error).toBeInstanceOf(RspytsPanicError);
    expect(error).not.toBeInstanceOf(HijackPanicError);
    expect((error as RspytsPanicError).message).toBe("kaboom 7");
  });

  it("maps the pre-registered staleHandle code", () => {
    const error = capture(() =>
      throwBridgeError(1, {
        code: "staleHandle",
        message: "handle has been dropped or never existed",
      }),
    );
    expect(error).toBeInstanceOf(StaleHandleError);
  });

  it("prefers registered classes and passes data through", () => {
    class WindowTooLargeError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "errTestWindowTooLarge", data);
      }
    }
    registerError("errTestWindowTooLarge", WindowTooLargeError);
    const error = capture(() =>
      throwBridgeError(1, {
        code: "errTestWindowTooLarge",
        message: "window too large",
        data: { max: 5 },
      }),
    );
    expect(error).toBeInstanceOf(WindowTooLargeError);
    expect(error).toBeInstanceOf(RspytsError);
    expect((error as RspytsError).data).toEqual({ max: 5 });
  });

  it("lets a later registration replace an earlier one", () => {
    class FirstError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "errTestReplaced", data);
      }
    }
    class SecondError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "errTestReplaced", data);
      }
    }
    registerError("errTestReplaced", FirstError);
    registerError("errTestReplaced", SecondError);
    const error = capture(() =>
      throwBridgeError(1, { code: "errTestReplaced", message: "m" }),
    );
    expect(error).toBeInstanceOf(SecondError);
  });

  it("falls back to RspytsError for unregistered codes", () => {
    const error = capture(() =>
      throwBridgeError(1, { code: "errTestNeverRegistered", message: "m", data: 1 }),
    );
    expect(error).toBeInstanceOf(RspytsError);
    expect(error).not.toBeInstanceOf(RspytsPanicError);
    expect((error as RspytsError).code).toBe("errTestNeverRegistered");
    expect((error as RspytsError).data).toBe(1);
  });

  it("tolerates malformed error payloads", () => {
    const error = capture(() => throwBridgeError(1, null));
    expect(error).toBeInstanceOf(RspytsError);
    expect((error as RspytsError).code).toBe("unknown");
    expect((error as RspytsError).message).toBe("unknown bridge error");
  });

  it("panics with fallback fields on a malformed status-2 payload", () => {
    for (const payload of [null, "just a string", 42]) {
      const error = capture(() => throwBridgeError(2, payload));
      expect(error).toBeInstanceOf(RspytsPanicError);
      expect((error as RspytsPanicError).message).toBe("unknown bridge error");
      expect((error as RspytsPanicError).code).toBe("panic");
    }
  });

  it('routes a status-1 error with code "panic" through the registry like any other', () => {
    // Precedence contrast with the status-2 test above: the panic CLASS is
    // reserved for status 2; the panic CODE at status 1 is ordinary.
    class PanicCodeError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "panic", data);
      }
    }
    registerError("panic", PanicCodeError);
    const error = capture(() => throwBridgeError(1, { code: "panic", message: "m" }));
    expect(error).toBeInstanceOf(PanicCodeError);
    expect(error).not.toBeInstanceOf(RspytsPanicError);
  });

  it("sets name to the concrete class on every thrown error", () => {
    class NamedTestError extends RspytsError {
      constructor(message: string, data?: unknown) {
        super(message, "errTestNamed", data);
      }
    }
    registerError("errTestNamed", NamedTestError);
    const cases: Array<{ status: number; code: string; name: string }> = [
      { status: 2, code: "whatever", name: "RspytsPanicError" },
      { status: 1, code: "staleHandle", name: "StaleHandleError" },
      { status: 1, code: "errTestNamed", name: "NamedTestError" },
      { status: 1, code: "errTestUnregistered", name: "RspytsError" },
    ];
    for (const { status, code, name } of cases) {
      const error = capture(() => throwBridgeError(status, { code, message: "m" }));
      expect((error as Error).name, `${status}/${code}`).toBe(name);
    }
  });
});
