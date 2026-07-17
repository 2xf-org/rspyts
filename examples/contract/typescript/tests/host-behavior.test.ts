import { describe, expect, it } from "vitest";

import init, {
  AnalyzeError,
  Analyzer,
  type ContractRequest,
  Quality,
  type Summary,
  summarize,
  validateRequest,
} from "@rspyts/acceptance";

const firstInitialization = init();
const secondInitialization = init();
if (firstInitialization !== secondInitialization) {
  throw new Error("concurrent init() calls did not share one cached promise");
}
await Promise.all([firstInitialization, secondInitialization]);

function assertGeneratedTypesAreDeeplyReadonly(
  request: ContractRequest,
  summary: Summary,
): void {
  // @ts-expect-error generated contract properties are readonly
  request.actor = "changed";
  // @ts-expect-error generated contract lists are readonly
  request.tags.push("changed");
  // @ts-expect-error nested generated contract properties are readonly
  summary.channel.id = "changed";
}

void assertGeneratedTypesAreDeeplyReadonly;

describe("generated browser package", () => {
  it("preserves nested types, typed arrays, and bytes", () => {
    const result = summarize(
      { id: "c3", sampleRateHz: 256 },
      new Float64Array([1, 2, 3]),
      new Uint8Array([1, 2, 3, 4]),
      2,
    );

    expect(result.count).toBe(3n);
    expect(result.average).toBe(2);
    expect(result.quality).toBe(Quality.Good);
    expect(result.normalized).toBeInstanceOf(Float64Array);
    expect([...result.normalized]).toEqual([2, 4, 6]);
    expect(result.fingerprint).toBeInstanceOf(Uint8Array);
    expect([...result.fingerprint]).toEqual([1, 2, 3, 4]);
    expect(Object.isFrozen(result)).toBe(true);
    expect(Object.isFrozen(result.channel)).toBe(true);
    expect(Object.isFrozen(result.normalized)).toBe(false);
    expect(Object.isFrozen(result.fingerprint)).toBe(false);
    result.normalized[0] = 4;
    result.fingerprint[0] = 4;
    expect(result.normalized[0]).toBe(4);
    expect(result.fingerprint[0]).toBe(4);
  });

  it("applies defaults, constraints, and aware RFC3339 datetimes", () => {
    const request = validateRequest({
      contractVersion: 2,
      actor: "viewer",
      occurredAt: "2026-07-16T12:34:56Z",
      tags: ["night"],
      quantity: undefined,
    });

    expect(request.quantity).toBe(1);
    expect(request.occurredAt).toBe("2026-07-16T12:34:56Z");
    expect(Object.isFrozen(request)).toBe(true);
    expect(Object.isFrozen(request.tags)).toBe(true);

    const valid: ContractRequest = {
      contractVersion: 2,
      actor: "viewer",
      occurredAt: "2026-07-16T12:34:56Z",
      tags: ["night"],
    };
    const invalidValues: ContractRequest[] = [
      { ...valid, contractVersion: 3 } as unknown as ContractRequest,
      { ...valid, actor: "" },
      { ...valid, quantity: 0 },
      { ...valid, occurredAt: "2026-07-16T12:34:56" },
      { ...valid, tags: [] },
    ];
    for (const invalid of invalidValues) {
      expect(() => validateRequest(invalid)).toThrow();
    }
  });

  it("preserves typed errors and deterministic resource disposal", () => {
    expect(() =>
      summarize(
        { id: "c3", sampleRateHz: 256 },
        new Float64Array(),
        new Uint8Array([1, 2, 3, 4]),
        1,
      ),
    ).toThrowError(AnalyzeError);

    const analyzer = new Analyzer({ id: "c3", sampleRateHz: 256 }, 0.5);
    expect(() =>
      analyzer.summarize(new Float64Array(), new Uint8Array([9, 8, 7, 6])),
    ).toThrowError(AnalyzeError);
    expect(analyzer.calls()).toBe(0n);
    const result = analyzer.summarize(
      new Float64Array([2, 4]),
      new Uint8Array([9, 8, 7, 6]),
    );
    expect(analyzer.calls()).toBe(1n);
    expect(result.normalized).toBeInstanceOf(Float64Array);
    expect([...result.normalized]).toEqual([1, 2]);
    analyzer.free();
    analyzer.free();
    expect(() => analyzer.calls()).toThrowError(/freed|closed/i);

    const disposable = new Analyzer({ id: "c3", sampleRateHz: 256 }, 1);
    disposable[Symbol.dispose]();
    disposable[Symbol.dispose]();
    expect(() => disposable.calls()).toThrowError(/freed|closed/i);
  });
});
