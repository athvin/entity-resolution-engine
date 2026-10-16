import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    include: ["tests/unit/**/*.test.{ts,tsx}", "tests/component/**/*.test.{ts,tsx}"],
    setupFiles: ["tests/setup.ts"],
    coverage: {
      // Measured + enforced in the NIGHTLY CI run only (never on the hot PR
      // path — see .github/workflows/frontend.yml). Scoped to src/lib/** on
      // purpose: that is the pure logic the vitest unit tier targets. Pages,
      // components, and BFF routes are exercised by the Playwright functional
      // tier, which is not instrumented here, so including them would report a
      // misleadingly low number and enforce nothing useful.
      provider: "v8",
      reporter: ["text-summary", "json-summary", "html"],
      reportsDirectory: "./coverage",
      include: ["src/lib/**/*.{ts,tsx}"],
      exclude: ["**/*.d.ts"],
      // Non-regression floor, set just below the current measurement. This is a
      // backstop against *regression*, not the quality bar — the real gate is the
      // per-PR behavioral suites (vitest units + the Playwright functional tier).
      // Ratchet these up as unit coverage grows; never down.
      thresholds: { lines: 28, functions: 15, statements: 29, branches: 35 },
    },
  },
  resolve: {
    alias: {
      "@": new URL("./src", import.meta.url).pathname,
      // Unit tests import server-only modules outside a React Server context.
      "server-only": new URL("./tests/server-only-stub.ts", import.meta.url).pathname,
    },
  },
});
