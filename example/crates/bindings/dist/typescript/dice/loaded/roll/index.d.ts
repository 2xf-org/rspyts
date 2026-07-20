/** The result of one loaded-die roll. */
export interface RollResult {
  readonly value: number;
  readonly favoredValue: number;
}

export function loadedRoll(value: number): RollResult;

export const DEFAULT_FAVORED_VALUE: number;
