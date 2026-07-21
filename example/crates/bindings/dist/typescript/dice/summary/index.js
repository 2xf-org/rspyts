import {
  native,
  nativeError,
  prepareHost,
  restoreHost,
} from "../../runtime.js";
import * as types_dice_fair_roll from "example/dice/fair/roll";

export function summarizeRoll(label, result) {
  try {
    const nativeResult = native["__rspyts_function_example_dice_5e9e146e9f6141a5"](prepareHost(label), prepareHost(result));
    return restoreHost(nativeResult, ["named", "example-dice::example_dice::summary::RollSummary"]);
  } catch (error) {
    throw nativeError(error, types_dice_fair_roll.RollError);
  }
}
