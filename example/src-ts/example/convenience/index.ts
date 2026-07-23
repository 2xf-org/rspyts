import { summarizeReadings } from "../api.js";

export function describeReadings(readings: ArrayLike<number>): string {
  const summary = summarizeReadings(Float64Array.from(readings));
  return `${summary.count} readings: ${summary.minimum.toFixed(2)} to ${summary.maximum.toFixed(2)} (mean ${summary.mean.toFixed(2)}, ${summary.trend})`;
}
