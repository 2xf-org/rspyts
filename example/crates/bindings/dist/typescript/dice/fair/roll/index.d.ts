/** A request to roll one type of die. */
export interface RollRequest {
  /** The number of sides on each die. */
  readonly sides: number;
  /** The number of dice to roll. */
  readonly count: number;
}
/** The result of a fair dice roll. */
export interface RollResult {
  readonly values: readonly number[];
  readonly total: bigint;
}

export class RollError extends globalThis.Error {
  readonly code: string;
  constructor(code: string, message: string);
}

export function rollDice(request: RollRequest, seed: bigint): RollResult;

export function rollValues(request: RollRequest, seed: bigint): Uint32Array;

export function seedFromBytes(bytes: Uint8Array): bigint;
export class DiceCup {
  constructor(sides: number, seed: bigint);
  roll(count: number): RollResult;
  close(): void;
}

export const DEFAULT_SEED: bigint;
