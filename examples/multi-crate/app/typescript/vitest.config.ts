import { defineConfig } from "vitest/config";

// Smoke-test scale: no coverage gate here — the basic example carries the
// thorough end-to-end suite, this package checks cross-package declaration
// compatibility and live calls.
export default defineConfig({
  test: {},
});
