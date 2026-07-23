import assert from "node:assert/strict";
import test from "node:test";

import {
  READINGS_FORMAT_VERSION,
  ReadingError,
  decodeReadings,
  describeReadings,
  encodeReadings,
  normalizeReadings,
  summarizeReadings,
} from "../build/example/index.js";

test("generated API and authored helper agree", () => {
  const readings = new Float64Array([12, 18, 15, 21]);

  assert.deepEqual(summarizeReadings(readings), {
    count: 4,
    minimum: 12,
    maximum: 21,
    mean: 16.5,
    trend: "rising",
  });
  assert.equal(
    describeReadings(readings),
    "4 readings: 12.00 to 21.00 (mean 16.50, rising)",
  );
  assert.equal(READINGS_FORMAT_VERSION, 1);
});

test("normalization returns a Float64Array", () => {
  const normalized = normalizeReadings(new Float64Array([10, 15, 20]));

  assert.ok(normalized instanceof Float64Array);
  assert.deepEqual(normalized, new Float64Array([0, 0.5, 1]));
});

test("portable format crosses byte and buffer boundaries", () => {
  const readings = new Float64Array([-1.25, 0, 42.5]);

  const encoded = encodeReadings(readings);

  assert.ok(encoded instanceof Uint8Array);
  assert.equal(new TextDecoder().decode(encoded.subarray(0, 8)), "RSPYTS01");
  assert.deepEqual(decodeReadings(encoded), readings);
});

test("domain errors retain their generated type and stable code", () => {
  for (const [operation, code, message] of [
    [() => summarizeReadings(new Float64Array()), "empty_readings", "at least one reading is required"],
    [() => summarizeReadings(new Float64Array([Number.NaN])), "non_finite_reading", "readings must contain only finite numbers"],
    [() => decodeReadings(new TextEncoder().encode("not readings")), "invalid_encoding", "invalid encoded readings"],
  ]) {
    assert.throws(operation, (error) => {
      assert.ok(error instanceof ReadingError);
      assert.equal(error.code, code);
      assert.equal(error.message, message);
      return true;
    });
  }
});
