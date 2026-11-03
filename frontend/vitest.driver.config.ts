import { defineConfig } from "vitest/config";

/**
 * The driver-contract tier: `tests/driver/**` against a REAL PostgreSQL.
 *
 * A separate config rather than a `projects` entry in `vitest.config.ts`,
 * because the one setting that matters here is the one that file cannot change.
 * `environment: "node"` is load-bearing, not stylistic: postgres v4 detects
 * binary with `x instanceof Uint8Array`, and under the default `jsdom` a Node
 * `Buffer` fails that check against jsdom's realm `Uint8Array` — the driver
 * then serialises it as text and PostgreSQL rejects it with
 * `22P02 invalid input syntax for type bytea`. That failure reads as a driver
 * incompatibility and is purely an artifact of the test environment. The
 * component tier genuinely needs jsdom, so the two cannot share a setting.
 *
 * `tests/driver/**` is outside `vitest.config.ts`'s `include`, so this tier is
 * invisible to `pnpm check` and cannot make the PR gate depend on a database
 * being up. It runs only via `pnpm test:driver`, which is the CI job and the
 * documented local command.
 *
 * No coverage block: the floor in `vitest.config.ts` is measured against the
 * unit tier, and instrumenting a five-case tier would move that number for
 * reasons unrelated to how much logic is covered.
 */
export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/driver/**/*.test.ts"],
    // One file, one database, order-dependent inserts — no reason to fork
    // workers, and a single process keeps the connection pool to one.
    fileParallelism: false,
    // The first case opens the pool and runs a round trip; on a cold CI
    // container that is slower than the 10s default is comfortable with, and
    // the same cold-path reasoning as `vitest.config.ts`'s hookTimeout applies.
    hookTimeout: 30_000,
  },
  resolve: {
    alias: {
      "@": new URL("./src", import.meta.url).pathname,
      // `src/lib/db` and `src/lib/db/crypto` are server-only modules imported
      // outside a React Server context, exactly as in the unit tier.
      "server-only": new URL("./tests/server-only-stub.ts", import.meta.url).pathname,
    },
  },
});
