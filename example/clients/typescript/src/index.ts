import {
  DEFAULT_SEED,
  DiceCup,
  RollError,
  rollValues,
  seedFromBytes,
  type RollResult,
} from "example/dice/fair/roll";
import {
  DiceCup as LoadedDiceCup,
  rollDice as loadedRollDice,
  type RollResult as LoadedRollResult,
} from "example/dice/loaded/roll";
import {
  summarizeRoll,
  type RollSummary,
} from "example/dice/summary";

export function rollThreeDice(): RollResult {
  const cup = new DiceCup(6, DEFAULT_SEED);
  try {
    return cup.roll(3);
  } finally {
    cup.close();
  }
}

console.log(rollThreeDice());

const seed = seedFromBytes(new TextEncoder().encode("rspyts"));
const values: Uint32Array = rollValues({ sides: 6, count: 3 }, seed);
console.log(values);

const loaded: LoadedRollResult = loadedRollDice(6);
const loadedCup = new LoadedDiceCup(6);
const loadedFromCup: LoadedRollResult = loadedCup.roll(3);
loadedCup.close();
const summary: RollSummary = summarizeRoll("fair", rollThreeDice());
console.log(loaded, loadedFromCup, summary);

try {
  summarizeRoll("", summary.result);
  throw new Error("an empty label must fail");
} catch (error) {
  if (!(error instanceof RollError) || error.code !== "empty_label") {
    throw error;
  }
}
