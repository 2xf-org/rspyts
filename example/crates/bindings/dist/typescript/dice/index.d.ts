import type * as types_dice_summary from "example/dice/summary";
/** A root report that references a model from a nested namespace. */
export interface DiceReport {
  readonly summary: types_dice_summary.RollSummary;
}
