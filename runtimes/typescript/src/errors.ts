/**
 * The rspyts error surface (ABI §5).
 *
 * A failed bridged call crosses the boundary as a JSON object
 * `{ code, message, data? }` inside a status-1 (application error) or
 * status-2 (panic) envelope. This module maps that object back onto a
 * JavaScript error class:
 *
 * - status 2 always becomes a {@link RspytsPanicError}, regardless of code;
 * - status 1 consults the code registry populated at import time by
 *   generated `errors.ts` modules (via {@link registerError}), falling back
 *   to the {@link RspytsError} base class for unregistered codes.
 */

import { STATUS_PANIC } from "./envelope.js";

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
 * Constructor shape a registered error class must have: `code` is baked
 * into the subclass, so only `message` and `data` arrive at throw time.
 */
export type BridgeErrorConstructor = new (message: string, data?: unknown) => RspytsError;

const registry = new Map<string, BridgeErrorConstructor>();

/**
 * Register an error class for a bridge error `code`. Generated `errors.ts`
 * modules call this at import time; a later registration for the same code
 * replaces the earlier one.
 */
export function registerError(code: string, ctor: BridgeErrorConstructor): void {
  registry.set(code, ctor);
}

/**
 * Throw the error class corresponding to a non-ok envelope: panic status
 * beats the registry; otherwise the registry maps `code` to a generated
 * class, with {@link RspytsError} as the fallback.
 */
export function throwBridgeError(status: number, payload: unknown): never {
  const body =
    payload !== null && typeof payload === "object"
      ? (payload as { code?: unknown; message?: unknown; data?: unknown })
      : {};
  const code = typeof body.code === "string" ? body.code : "unknown";
  const message = typeof body.message === "string" ? body.message : "unknown bridge error";
  if (status === STATUS_PANIC) {
    throw new RspytsPanicError(message, body.data);
  }
  const ctor = registry.get(code);
  if (ctor !== undefined) {
    throw new ctor(message, body.data);
  }
  throw new RspytsError(message, code, body.data);
}

// `staleHandle` is raised by the bridge itself (ABI §8), not by generated
// error enums, so the runtime pre-registers it.
registerError("staleHandle", StaleHandleError);
