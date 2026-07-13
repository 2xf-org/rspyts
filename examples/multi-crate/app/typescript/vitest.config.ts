import { defineConfig } from "vitest/config";

// Smoke-test scale: no coverage gate here — the basic example carries the
// thorough end-to-end suite, this package only proves cross-package type
// identity.
export default defineConfig({
  test: {},
});
