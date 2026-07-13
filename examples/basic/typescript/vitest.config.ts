import { defineConfig } from "vitest/config";

/**
 * Coverage is opt-in: plain `npm test` stays fast, `npm run test:cov`
 * passes `--coverage` which flips `coverage.enabled` on. The include
 * list scopes measurement to this package's (generated) sources; the
 * runtime package enforces its own thresholds.
 */
export default defineConfig({
  test: {
    coverage: {
      provider: "v8",
      include: ["src/**"],
      reporter: ["text"],
      thresholds: {
        lines: 85,
      },
    },
  },
});
