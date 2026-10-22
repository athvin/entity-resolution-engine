import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    include: ["tests/unit/**/*.test.{ts,tsx}", "tests/component/**/*.test.{ts,tsx}"],
    setupFiles: ["tests/setup.ts"],
    // The DB-backed files open PGlite and run every drizzle migration in a
    // `beforeAll`, which the 10s default is uncomfortably close to on a cold
    // runner: `bff-members.test.ts`'s hook was measured at 10202 ms in CI
    // against that default, while it is ~1.2 s warm here. A hook that trips the
    // limit fails its whole FILE and reports its cases as `skipped` rather than
    // failed — the run does exit non-zero, so the gate blocks rather than lying,
    // but the message points at nothing and the redness is cache luck. 30 s is
    // slack for the cold path only: a hook that is genuinely stuck still fails,
    // just later.
    hookTimeout: 30_000,
    coverage: {
      // Measured + enforced on EVERY PR: `pnpm check` runs with --coverage, so
      // the floor below is part of the same gate as lint and tsc. (The nightly
      // job in .github/workflows/frontend.yml still runs, but only to publish
      // the browsable HTML report as an artifact.) v8 instrumentation of this
      // tier costs under a second, so there is no latency argument for deferring
      // it to the next morning.
      //
      // Scoped to src/lib/** on purpose: that is the logic the vitest tier
      // targets. Pages, components, and BFF routes are exercised by the
      // Playwright functional tier, which is not instrumented here, so including
      // them would report a misleadingly low number and enforce nothing useful.
      provider: "v8",
      reporter: ["text-summary", "json-summary", "html"],
      reportsDirectory: "./coverage",
      include: ["src/lib/**/*.{ts,tsx}"],
      exclude: ["**/*.d.ts"],
      // Non-regression floor, set just below the current measurement (lines
      // 49.3, functions 34.9, statements 49.4, branches 50.1). A backstop
      // against *regression*, not the quality bar — the real gate is the
      // behavioral suites. Ratchet up as coverage grows; never down.
      //
      // Why this is not ~70%: roughly 1,300 of src/lib's lines are
      // src/lib/query/** — TanStack Query hooks that need a QueryClient and a
      // backend to say anything, and which the Playwright tier already drives.
      // Chasing them in jsdom would raise the number without raising confidence.
      thresholds: { lines: 48, functions: 33, statements: 48, branches: 49 },
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
