import { defineConfig } from "vitest/config";

/**
 * Coverage is opt-in: plain `npm test` stays fast, `npm run test:cov`
 * passes `--coverage` which flips `coverage.enabled` on and enforces
 * the thresholds below.
 */
export default defineConfig({
  test: {
    coverage: {
      provider: "v8",
      include: ["src/**"],
      reporter: ["text"],
      thresholds: {
        lines: 90,
        functions: 90,
        statements: 90,
        branches: 85,
      },
    },
  },
});
