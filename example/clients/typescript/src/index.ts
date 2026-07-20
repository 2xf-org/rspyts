import {
  DEFAULT_SEED,
  DiceCup,
  rollValues,
  seedFromBytes,
  type RollResult,
} from "example";

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
