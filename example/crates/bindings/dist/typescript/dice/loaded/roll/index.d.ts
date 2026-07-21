/** The result of one loaded-die roll. */
export interface RollResult {
  readonly value: number;
  readonly favoredValue: number;
}

export function rollDice(value: number): RollResult;
export class DiceCup {
  constructor(favoredValue: number);
  roll(value: number): RollResult;
  close(): void;
}

export const DEFAULT_FAVORED_VALUE: number;
