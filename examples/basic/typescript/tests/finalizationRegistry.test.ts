import type { BridgeModule } from "rspyts";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

const bridgeMocks = vi.hoisted(() => ({
  callDrop: vi.fn(),
  callFn: vi.fn(() => ({ value: 1, tail: new Uint8Array() })),
  verifyModuleContract: vi.fn(),
}));

vi.mock("rspyts/internal/abi3", async (importOriginal) => ({
  ...(await importOriginal<typeof import("rspyts/internal/abi3")>()),
  callDrop: bridgeMocks.callDrop,
  callFn: bridgeMocks.callFn,
  verifyModuleContract: bridgeMocks.verifyModuleContract,
}));

let createClient: typeof import("../src/generated/client.js").createClient;

beforeAll(async () => {
  vi.stubGlobal("FinalizationRegistry", undefined);
  vi.resetModules();
  ({ createClient } = await import("../src/generated/client.js"));
});

afterAll(() => {
  vi.unstubAllGlobals();
});

describe("generated clients without FinalizationRegistry", () => {
  it("loads and keeps explicit disposal working", () => {
    const client = createClient({} as BridgeModule);
    const counter = new client.Counter(1, "portable");

    counter.free();
    counter[Symbol.dispose]();

    expect(bridgeMocks.callDrop).toHaveBeenCalledTimes(2);
  });
});
