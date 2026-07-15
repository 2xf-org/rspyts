import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BridgeModule } from "rspyts";

const callFn = vi.hoisted(() => vi.fn());
const verifyModuleContract = vi.hoisted(() => vi.fn());

vi.mock("rspyts/internal/abi3", async (importOriginal) => ({
  ...(await importOriginal<typeof import("rspyts/internal/abi3")>()),
  callFn,
  verifyModuleContract,
}));

import { createClient } from "../src";

const client = createClient({} as BridgeModule);

beforeEach(() => {
  callFn.mockReset();
});

describe("generated successful-return validation", () => {
  it("rejects stale scalar and handle wire values", () => {
    callFn.mockReturnValueOnce({ value: "2.5", tail: new Uint8Array() });
    expect(() => client.roundValue(2.5, "nearest")).toThrow(/finite JSON number/);

    callFn.mockReturnValueOnce({ value: "1", tail: new Uint8Array() });
    expect(() => new client.Counter(0, "test")).toThrow(/JSON integer/);
  });

  it("rejects malformed tuples, objects, enums, and buffer dtypes", () => {
    callFn.mockReturnValueOnce({ value: ["1", "2", "3"], tail: new Uint8Array() });
    expect(() => client.echoExactPair([1n, 2n])).toThrow(/array of length 2/);

    callFn.mockReturnValueOnce({
      value: { signed: "1", pair: ["1", "2"], history: [] },
      tail: new Uint8Array(),
    });
    expect(() =>
      client.echoExactNumbers({
        signed: 1n,
        unsigned: 2n,
        transferId: 3n,
        pair: [1n, 2n],
        history: [],
        byName: {},
      }),
    ).toThrow(/required wire field.*unsigned/);

    callFn.mockReturnValueOnce({ value: { type: "stale" }, tail: new Uint8Array() });
    expect(() => client.echoTransferState({ type: "pending" })).toThrow(/discriminator/);

    callFn.mockReturnValueOnce({ value: new Int8Array([1]), tail: new Uint8Array() });
    expect(() => client.echoBytes(new Uint8Array([1]))).toThrow(/buffer attachment/);

    callFn.mockReturnValueOnce({
      value: {
        id: 1,
        payload: new Uint8Array([1]),
        samples: new Float32Array([1]),
        chunks: [],
        channels: {},
      },
      tail: new Uint8Array(),
    });
    expect(() =>
      client.echoBinaryPacket({
        id: 1,
        payload: new Uint8Array([1]),
        samples: new Float64Array([1]),
        chunks: [],
        channels: {},
      }),
    ).toThrow(/buffer attachment/);
  });

  it("keeps Json transparent at its schema-declared return position", () => {
    const collision = { __rspyts_buf__: { off: 0, len: 1, dt: "u8" } };
    callFn.mockReturnValueOnce({ value: collision, tail: new Uint8Array() });
    expect(client.annotate(1, {})).toEqual(collision);
  });
});
