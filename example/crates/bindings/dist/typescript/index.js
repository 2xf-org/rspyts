import {
  native,
  nativeError,
  prepareHost,
  restoreHost,
} from "./runtime.js";

export class RollError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "RollError";
    this.code = code;
  }
}

export function rollDice(request, seed) {
  try {
    const result = native["__rspyts_export_rollDice"](prepareHost(request), prepareHost(seed));
    return restoreHost(result, ["named", "RollResult"]);
  } catch (error) {
    throw nativeError(error, RollError);
  }
}

export function rollValues(request, seed) {
  try {
    const result = native["__rspyts_export_rollValues"](prepareHost(request), prepareHost(seed));
    return restoreHost(result, ["buffer", "u32"]);
  } catch (error) {
    throw nativeError(error, RollError);
  }
}

export function seedFromBytes(bytes) {
  const result = native["__rspyts_export_seedFromBytes"](prepareHost(bytes));
  return restoreHost(result, null);
}

export class DiceCup {
  constructor(sides, seed) {
    try {
      this.nativeResource = new native.RspytsWasmDiceCup(prepareHost(sides), prepareHost(seed));
    } catch (error) {
      throw nativeError(error, RollError);
    }
  }

  roll(count) {
    try {
      const result = this.nativeResource.roll(prepareHost(count));
      return restoreHost(result, ["named", "RollResult"]);
    } catch (error) {
      throw nativeError(error, RollError);
    }
  }

  close() {
    this.nativeResource.close();
  }
}

export const DEFAULT_SEED = restoreHost(42n, null);
