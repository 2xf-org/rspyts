import {
  native,
  prepareHost,
  restoreHost,
} from "../../../runtime.js";

export function rollDice(value) {
  const nativeResult = native["__rspyts_function_example_dice_3af246b4f17bbfac"](prepareHost(value));
  return restoreHost(nativeResult, ["named", "example-dice::example_dice::loaded::roll::RollResult"]);
}

export class DiceCup {
  constructor(favoredValue) {
    this.nativeResource = new native["__rspyts_resource_example_dice_e5e96c8d1c3073b6"](prepareHost(favoredValue));
  }

  roll(value) {
    const nativeResult = this.nativeResource.roll(prepareHost(value));
    return restoreHost(nativeResult, ["named", "example-dice::example_dice::loaded::roll::RollResult"]);
  }

  close() {
    this.nativeResource.close();
  }
}

export const DEFAULT_FAVORED_VALUE = restoreHost(6, null);
