import {
  native,
  prepareHost,
  restoreHost,
} from "../../../runtime.js";

export function loadedRoll(value) {
  const nativeResult = native["__rspyts_export_loadedRoll"](prepareHost(value));
  return restoreHost(nativeResult, ["named", "example-dice::example_dice::loaded::roll::RollResult"]);
}

export const DEFAULT_FAVORED_VALUE = restoreHost(6, null);
