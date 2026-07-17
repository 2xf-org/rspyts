import { describe, expect, it } from "vitest";

import init, {
  type BatchOptions,
  type Calculation,
  CalculationError,
  Calculator,
  Magnitude,
  SAFE_WIDE,
  UNSAFE_WIDE,
  type VectorSpec,
  calculate,
  validateBatchOptions,
} from "@example/owner";
import * as wire from "@example/owner/wire";

const firstInitialization = init();
const secondInitialization = init();
if (firstInitialization !== secondInitialization) {
  throw new Error("concurrent init() calls did not share one cached promise");
}
await Promise.all([firstInitialization, secondInitialization]);

function assertGeneratedTypesAreDeeplyReadonly(
  options: BatchOptions,
  calculation: Calculation,
): void {
  // @ts-expect-error generated contract properties are readonly
  options.label = "changed";
  // @ts-expect-error generated contract lists are readonly
  options.groups.push("changed");
  // @ts-expect-error nested generated contract properties are readonly
  calculation.vector.name = "changed";
}

void assertGeneratedTypesAreDeeplyReadonly;

function assertWireConstantTypes(): void {
  const safe: number = wire.SAFE_WIDE;
  void safe;
  // @ts-expect-error values outside Number.isSafeInteger are omitted from ./wire
  void wire.UNSAFE_WIDE;
}

void assertWireConstantTypes;

describe("generated browser package", () => {
  it("keeps wide integers exact without leaking unsafe values onto the wire", () => {
    expect(SAFE_WIDE).toBe(9_007_199_254_740_991n);
    expect(UNSAFE_WIDE).toBe(9_007_199_254_740_992n);
    expect(wire.SAFE_WIDE).toBe(9_007_199_254_740_991);
    expect("UNSAFE_WIDE" in wire).toBe(false);
  });

  it("preserves nested types, typed arrays, and bytes", () => {
    const vector: VectorSpec = { name: "example", dimensions: 3 };
    const result = calculate(
      vector,
      new Float64Array([1, 2, 3]),
      new Uint8Array([1, 2, 3, 4]),
      2,
    );

    expect(result.count).toBe(3);
    expect(result.mean).toBe(2);
    expect(result.magnitude).toBe(Magnitude.Regular);
    expect(result.scaled).toBeInstanceOf(Float64Array);
    expect([...result.scaled]).toEqual([2, 4, 6]);
    expect(result.checksum).toBeInstanceOf(Uint8Array);
    expect([...result.checksum]).toEqual([1, 2, 3, 4]);
    expect(Object.isFrozen(result)).toBe(true);
    expect(Object.isFrozen(result.vector)).toBe(true);
    expect(Object.isFrozen(result.scaled)).toBe(false);
    expect(Object.isFrozen(result.checksum)).toBe(false);
    result.scaled[0] = 4;
    result.checksum[0] = 4;
    expect(result.scaled[0]).toBe(4);
    expect(result.checksum[0]).toBe(4);
  });

  it("applies defaults, constraints, and aware RFC3339 datetimes", () => {
    const options = validateBatchOptions({
      schemaVersion: 1,
      label: "example",
      createdAt: "2030-01-02T03:04:05Z",
      groups: ["primary"],
      attempts: undefined,
    });

    expect(options.attempts).toBe(1);
    expect(options.createdAt).toBe("2030-01-02T03:04:05Z");
    expect(Object.isFrozen(options)).toBe(true);
    expect(Object.isFrozen(options.groups)).toBe(true);

    const valid: BatchOptions = {
      schemaVersion: 1,
      label: "example",
      createdAt: "2030-01-02T03:04:05Z",
      groups: ["primary"],
    };
    const invalidValues: BatchOptions[] = [
      { ...valid, schemaVersion: 2 } as unknown as BatchOptions,
      { ...valid, label: "" },
      { ...valid, attempts: 0 },
      { ...valid, createdAt: "2030-01-02T03:04:05" },
      { ...valid, groups: [] },
    ];
    for (const invalid of invalidValues) {
      expect(() => validateBatchOptions(invalid)).toThrow();
    }
  });

  it("preserves typed errors and deterministic resource cleanup", () => {
    expect(() =>
      calculate(
        { name: "example", dimensions: 3 },
        new Float64Array(),
        new Uint8Array([1, 2, 3, 4]),
        1,
      ),
    ).toThrowError(CalculationError);

    for (const checksum of [
      new Uint8Array([1, 2, 3]),
      new Uint8Array([1, 2, 3, 4, 5]),
    ]) {
      expect(() =>
        calculate(
          { name: "example", dimensions: 3 },
          new Float64Array([1]),
          checksum,
          1,
        ),
      ).toThrowError(/4-byte array/);
    }

    const calculator = new Calculator({ name: "example", dimensions: 3 }, 0.5);
    expect(() =>
      calculator.calculate(new Float64Array(), new Uint8Array([9, 8, 7, 6])),
    ).toThrowError(CalculationError);
    expect(calculator.calls()).toBe(0n);
    const result = calculator.calculate(
      new Float64Array([2, 4]),
      new Uint8Array([9, 8, 7, 6]),
    );
    expect(calculator.calls()).toBe(1n);
    expect(result.scaled).toBeInstanceOf(Float64Array);
    expect([...result.scaled]).toEqual([1, 2]);
    calculator.free();
    calculator.free();
    expect(() => calculator.calls()).toThrowError(/freed|closed/i);

    const explicitlyFreed = new Calculator({ name: "example", dimensions: 3 }, 1);
    explicitlyFreed.free();
    explicitlyFreed.free();
    expect(() => explicitlyFreed.calls()).toThrowError(/freed|closed/i);
  });
});
