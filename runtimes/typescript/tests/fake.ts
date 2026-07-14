/**
 * A fake BridgeModule for tests: real `WebAssembly.Memory`, a JS bump
 * allocator implementing `rspyts_alloc`/`rspyts_free` with exact-length
 * bookkeeping, and test-installed export functions that write pre-built
 * envelopes into linear memory — no compiled wasm needed.
 */

import { HEADER_LEN } from "../src/envelope.js";
import type { BridgeModule } from "../src/module.js";

export interface FakeOptions {
  /** Grow memory by one page on every allocation, detaching every
   * previously-created ArrayBuffer view — simulates worst-case growth. */
  growOnAlloc?: boolean;
  /** Zero the freed byte range inside rspyts_free, before the runtime can
   * possibly look at it again — proves the runtime copies data out BEFORE
   * freeing, not merely before some later mutation. */
  zeroOnFree?: boolean;
}

export interface Fake {
  mod: BridgeModule;
  memory: WebAssembly.Memory;
  /** ptr → allocated length for every live allocation. Empty after a
   * leak-free call. */
  live: Map<number, number>;
  /** Every (ptr, len) passed to rspyts_free, in order. */
  freed: Array<{ ptr: number; len: number }>;
  alloc: (len: number) => number;
  /** Advance the bump pointer by `len` bytes WITHOUT recording an
   * allocation — knocks subsequent allocations off alignment without
   * registering as a leak in the ledger. */
  misalign: (len: number) => void;
  setExport: (name: string, fn: Function) => void;
  /** Write an envelope into fake linear memory (via the fake allocator,
   * so free bookkeeping applies) and return its pointer. */
  putEnvelope: (status: number, payload: unknown, tail?: Uint8Array) => number;
  /** Copy `len` bytes out of linear memory. */
  readBytes: (ptr: number, len: number) => Uint8Array;
  /** Throw unless every allocation was released and every non-empty
   * pointer was freed exactly once. The bump allocator never reuses
   * addresses, so pointer uniqueness across the whole history is exact. */
  assertAllFreedOnce: () => void;
}

/** Serialize an envelope per ABI §4 (12-byte LE header, JSON, raw tail). */
export function buildEnvelope(
  status: number,
  json: string,
  tail: Uint8Array = new Uint8Array(0),
): Uint8Array {
  const jsonBytes = new TextEncoder().encode(json);
  const out = new Uint8Array(HEADER_LEN + jsonBytes.byteLength + tail.byteLength);
  const view = new DataView(out.buffer);
  view.setUint8(0, status);
  view.setUint32(4, jsonBytes.byteLength, true);
  view.setUint32(8, tail.byteLength, true);
  out.set(jsonBytes, HEADER_LEN);
  out.set(tail, HEADER_LEN + jsonBytes.byteLength);
  return out;
}

export function createFake(options: FakeOptions = {}): Fake {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const live = new Map<number, number>();
  const freed: Array<{ ptr: number; len: number }> = [];
  // Start past address 0 and advance by exact byte lengths, so
  // consecutive allocations are packed and frequently misaligned — just
  // like the real alignment-1 rspyts_alloc.
  let top = 8;

  const alloc = (len: number): number => {
    if (options.growOnAlloc) {
      memory.grow(1);
    }
    if (len === 0) {
      return 1; // dangling non-null, mirrors rspyts_alloc(0)
    }
    while (top + len > memory.buffer.byteLength) {
      memory.grow(1);
    }
    const ptr = top;
    top += len;
    live.set(ptr, len);
    return ptr;
  };

  const free = (ptr: number, len: number): void => {
    freed.push({ ptr, len });
    if (len === 0) {
      return;
    }
    const allocated = live.get(ptr);
    if (allocated === undefined) {
      throw new Error(`fake: rspyts_free of unknown pointer ${ptr}`);
    }
    if (allocated !== len) {
      throw new Error(
        `fake: rspyts_free length mismatch at ${ptr}: allocated ${allocated}, freed ${len}`,
      );
    }
    live.delete(ptr);
    if (options.zeroOnFree) {
      new Uint8Array(memory.buffer, ptr, len).fill(0);
    }
  };

  const exportsObj: Record<string, unknown> = {
    memory,
    rspyts_abi_version: () => 2,
    rspyts_alloc: alloc,
    rspyts_free: free,
  };

  const mod: BridgeModule = {
    exports: exportsObj as unknown as BridgeModule["exports"],
    memory,
  };

  return {
    mod,
    memory,
    live,
    freed,
    alloc,
    misalign: (len) => {
      top += len;
    },
    setExport: (name, fn) => {
      exportsObj[name] = fn;
    },
    putEnvelope: (status, payload, tail) => {
      const bytes = buildEnvelope(status, JSON.stringify(payload), tail);
      const ptr = alloc(bytes.byteLength);
      new Uint8Array(memory.buffer, ptr, bytes.byteLength).set(bytes);
      return ptr;
    },
    readBytes: (ptr, len) => new Uint8Array(memory.buffer, ptr, len).slice(),
    assertAllFreedOnce: () => {
      if (live.size > 0) {
        const leaked = Array.from(live.entries())
          .map(([ptr, len]) => `${ptr} (${len} bytes)`)
          .join(", ");
        throw new Error(`fake: leaked allocations: ${leaked}`);
      }
      const seen = new Set<number>();
      for (const { ptr, len } of freed) {
        if (len === 0) {
          continue;
        }
        if (seen.has(ptr)) {
          throw new Error(`fake: pointer ${ptr} freed more than once`);
        }
        seen.add(ptr);
      }
    },
  };
}

/** A real WebAssembly trap-shaped error for poison lifecycle tests. */
export function runtimeTrap(message = "unreachable executed"): WebAssembly.RuntimeError {
  return new WebAssembly.RuntimeError(message);
}
