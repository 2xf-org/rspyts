import {
  native,
  nativeError,
  prepareHost,
  restoreHost,
} from "../../../runtime.js";

export const RollMode = Object.freeze({
  Fast: "fast",
  Safe: "safe",
});

export class RollError extends globalThis.Error {
  constructor(code, message) {
    super(message);
    this.name = "RollError";
    this.code = code;
  }
}

export function rollDice(request, seed) {
  try {
    const nativeResult = native["__rspyts_function_example_dice_3f04e55579ef1c90"](prepareHost(request), prepareHost(seed));
    return restoreHost(nativeResult, ["named", "example-dice::example_dice::fair::roll::RollResult"]);
  } catch (error) {
    throw nativeError(error, RollError);
  }
}

export function rollValues(request, seed) {
  try {
    const nativeResult = native["__rspyts_function_example_dice_79ab58465b7e5143"](prepareHost(request), prepareHost(seed));
    return restoreHost(nativeResult, ["buffer", "u32"]);
  } catch (error) {
    throw nativeError(error, RollError);
  }
}

export function seedFromBytes(bytes) {
  const nativeResult = native["__rspyts_function_example_dice_5026eacb605dfd00"](prepareHost(bytes));
  return restoreHost(nativeResult, null);
}

export class DiceCup {
  constructor(sides, seed) {
    try {
      this.nativeResource = new native["__rspyts_resource_example_dice_e8caefb21754b303"](prepareHost(sides), prepareHost(seed));
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
