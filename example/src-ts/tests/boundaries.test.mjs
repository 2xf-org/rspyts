import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import test from "node:test";

import {
  decodeReadings,
  encodeReadings,
  normalizeReadings,
} from "../build/example/index.js";

test("large typed arrays stay on direct Wasm boundaries", () => {
  const count = 4 * 1024 * 1024;
  const readings = new Float64Array(count);
  readings[0] = -1;
  readings[count - 1] = 1;
  const limit = Number(process.env.RSPYTS_PERFORMANCE_LIMIT_MS ?? 5_000);

  let started = performance.now();
  const normalized = normalizeReadings(readings);
  const normalizationMilliseconds = performance.now() - started;

  started = performance.now();
  const encoded = encodeReadings(readings);
  const encodingMilliseconds = performance.now() - started;

  started = performance.now();
  const decoded = decodeReadings(encoded);
  const decodingMilliseconds = performance.now() - started;

  assert.ok(normalized instanceof Float64Array);
  assert.equal(normalized.length, count);
  assert.ok(encoded instanceof Uint8Array);
  assert.equal(encoded.byteLength, readings.byteLength + 12);
  assert.ok(decoded instanceof Float64Array);
  assert.equal(decoded.length, count);
  assert.equal(decoded[0], -1);
  assert.equal(decoded[count - 1], 1);
  assert.ok(normalizationMilliseconds < limit, `normalization took ${normalizationMilliseconds.toFixed(1)} ms`);
  assert.ok(encodingMilliseconds < limit, `encoding took ${encodingMilliseconds.toFixed(1)} ms`);
  assert.ok(decodingMilliseconds < limit, `decoding took ${decodingMilliseconds.toFixed(1)} ms`);
});
