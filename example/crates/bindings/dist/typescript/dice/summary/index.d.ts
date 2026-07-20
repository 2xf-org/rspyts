import type * as types_dice_fair_roll from "example/dice/fair/roll";
/** A named summary of one fair roll. */
export interface RollSummary {
  readonly label: string;
  readonly result: types_dice_fair_roll.RollResult;
}

export function summarizeRoll(label: string, result: types_dice_fair_roll.RollResult): RollSummary;
