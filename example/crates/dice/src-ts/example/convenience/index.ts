import type { RollResult } from "../dice/fair/roll/models.js";

export function averageRoll(result: RollResult): number {
  return Number(result.total) / result.values.length;
}
