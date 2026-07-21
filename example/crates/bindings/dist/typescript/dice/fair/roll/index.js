import {
  native,
  nativeError,
  prepareHost,
  restoreHost,
} from "../../../runtime.js";

export class RollError extends globalThis.Error {
  constructor(code, message) {
    super(message);
    this.name = "RollError";
    this.code = code;
  }
}

export function rollDice(request, seed) {
  try {
    const nativeResult = native["__rspyts_function_example_dice_f230a4119ceb9b1d"](prepareHost(request), prepareHost(seed));
    return restoreHost(nativeResult, ["named", "example-dice::example_dice::fair::roll::RollResult"]);
  } catch (error) {
    throw nativeError(error, RollError);
  }
}

export function rollValues(request, seed) {
  try {
    const nativeResult = native["__rspyts_function_example_dice_7da6db2d088cd0a5"](prepareHost(request), prepareHost(seed));
    return restoreHost(nativeResult, ["buffer", "u32"]);
  } catch (error) {
    throw nativeError(error, RollError);
  }
}

export function seedFromBytes(bytes) {
  const nativeResult = native["__rspyts_function_example_dice_d05b90e269d62a6d"](prepareHost(bytes));
  return restoreHost(nativeResult, null);
}

export class DiceCup {
  constructor(sides, seed) {
    try {
      this.nativeResource = new native["__rspyts_resource_example_dice_618e979366d8f5d6"](prepareHost(sides), prepareHost(seed));
    } catch (error) {
      throw nativeError(error, RollError);
    }
  }

  roll(count) {
    try {
      const nativeResult = this.nativeResource.roll(prepareHost(count));
      return restoreHost(nativeResult, ["named", "example-dice::example_dice::fair::roll::RollResult"]);
    } catch (error) {
      throw nativeError(error, RollError);
    }
  }

  close() {
    this.nativeResource.close();
  }
}

export const DEFAULT_SEED = restoreHost(42n, null);
