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

import { STATUS_ERROR, STATUS_PANIC } from "./envelope.js";

/**
 * Base class for every error crossing the rspyts bridge.
 *
 * Generated error classes extend this and bake in their `code`; `message`
 * is the human-readable text from Rust and `data` carries the error
 * variant's named fields (or `undefined`).
 */
export class RspytsError extends Error {
  code: string;
  data: unknown;

  constructor(message: string, code: string, data?: unknown) {
    super(message);
    // `new.target` gives subclasses their own name without each one
    // having to set it.
    this.name = new.target.name;
    this.code = code;
    this.data = data;
  }
}

/** A Rust panic was caught behind the shim (envelope status 2, ABI §9). */
export class RspytsPanicError extends RspytsError {
  constructor(message: string, data?: unknown) {
    super(message, "panic", data);
  }
}

/** A method was called on a dropped or unknown handle (ABI §8). */
export class StaleHandleError extends RspytsError {
  constructor(message: string, data?: unknown) {
    super(message, "staleHandle", data);
  }
}

/**
 * A prior WebAssembly runtime trap left this module instance unsafe to
 * call. Instantiate a fresh module before retrying.
 */
export class InstancePoisonedError extends Error {
  constructor() {
    super(
      "rspyts: WebAssembly instance is poisoned after a runtime trap; instantiate a fresh module",
    );
    this.name = "InstancePoisonedError";
  }
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
export function throwBridgeError(
  status: number,
  payload: unknown,
  errorTypes?: BridgeErrorRegistry,
): never {
  if (status !== STATUS_ERROR && status !== STATUS_PANIC) {
    throw new TypeError(`rspyts: bridge error status must be 1 or 2, got ${status}`);
  }
  const body = validateErrorPayload(payload);
  const { code, message } = body;
  if (status === STATUS_PANIC) {
    if (code !== "panic") {
      throw new TypeError(`rspyts: panic envelope must use error code "panic", got ${JSON.stringify(code)}`);
    }
    throw new RspytsPanicError(message, body.data);
  }
  const scopedCtor =
    errorTypes !== undefined && Object.prototype.hasOwnProperty.call(errorTypes, code)
      ? errorTypes[code]
      : undefined;
  const ctor: unknown = code === "staleHandle" ? StaleHandleError : scopedCtor;
  if (ctor !== undefined) {
    if (typeof ctor !== "function") {
      throw new TypeError(
        `rspyts: error registry entry for ${JSON.stringify(code)} must be an error constructor`,
      );
    }
    let error: unknown;
    try {
      error = new (ctor as BridgeErrorConstructor)(message, body.data);
    } catch (cause) {
      throw new TypeError(
        `rspyts: error registry entry for ${JSON.stringify(code)} is not a usable constructor`,
        { cause },
      );
    }
    if (!(error instanceof RspytsError)) {
      throw new TypeError(
        `rspyts: error registry entry for ${JSON.stringify(code)} must construct RspytsError`,
      );
    }
    throw error;
  }
  throw new RspytsError(message, code, body.data);
}

function validateErrorPayload(
  payload: unknown,
): { code: string; message: string; data?: unknown } {
  if (payload === null || typeof payload !== "object" || Array.isArray(payload)) {
    throw new TypeError("rspyts: error envelope JSON must be an object");
  }
  const prototype = Object.getPrototypeOf(payload);
  if (prototype !== Object.prototype && prototype !== null) {
    throw new TypeError("rspyts: error envelope JSON must be a plain object");
  }
  const body = payload as Record<string, unknown>;
  const keys = Object.keys(body);
  for (const required of ["code", "message"]) {
    if (!Object.prototype.hasOwnProperty.call(body, required)) {
      throw new TypeError(`rspyts: error envelope is missing required field ${JSON.stringify(required)}`);
    }
  }
  for (const key of keys) {
    if (key !== "code" && key !== "message" && key !== "data") {
      throw new TypeError(`rspyts: error envelope has unexpected field ${JSON.stringify(key)}`);
    }
  }
  if (typeof body["code"] !== "string" || body["code"].length === 0) {
    throw new TypeError("rspyts: error envelope code must be a non-empty string");
  }
  if (typeof body["message"] !== "string") {
    throw new TypeError("rspyts: error envelope message must be a string");
  }
  return {
    code: body["code"],
    message: body["message"],
    ...(Object.prototype.hasOwnProperty.call(body, "data") ? { data: body["data"] } : {}),
  };
}
