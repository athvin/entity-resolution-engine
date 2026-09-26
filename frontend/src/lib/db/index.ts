import "server-only";
import type { PgDatabase, PgQueryResultHKT } from "drizzle-orm/pg-core";

import { env } from "@/lib/env";
import * as schema from "./schema";

/**
 * The BFF's database handle. Two drivers behind one type:
 *
 * - postgres.js against ERWEB_DATABASE_URL (dev/prod — the control-plane
 *   Postgres, schema `erweb`).
 * - PGlite (in-process WASM Postgres) when the URL is `pglite://memory` — the
 *   mock-backed Playwright tier runs the REAL session/auth/vault code paths
 *   with no Docker. Migrations apply on first use; with ERWEB_TEST_FIXTURES=1
 *   the deterministic test users/orgs are inserted too.
 */
export type Db = PgDatabase<PgQueryResultHKT, typeof schema>;

// Stashed on globalThis, not at module level: Next.js keeps separate module
// graphs for server components and route handlers, and a per-graph memo would
// mean two connection pools — or, under PGlite, two different in-memory
// databases (sessions written by a route handler invisible to every layout).
const globalStash = globalThis as { __erwebDb?: Promise<Db> };

async function connect(): Promise<Db> {
  const url = env().ERWEB_DATABASE_URL;
  if (url.startsWith("pglite:")) {
    const { PGlite } = await import("@electric-sql/pglite");
    const { drizzle } = await import("drizzle-orm/pglite");
    const { migrate } = await import("drizzle-orm/pglite/migrator");
    const client = new PGlite();
    const db = drizzle(client, { schema });
    await migrate(db, { migrationsFolder: "drizzle" });
    if (process.env.ERWEB_TEST_FIXTURES === "1") {
      const { installTestFixtures } = await import("./test-fixtures");
      await installTestFixtures(db);
    }
    return db;
  }
  const { drizzle } = await import("drizzle-orm/postgres-js");
  const { default: postgres } = await import("postgres");
  const client = postgres(url, { max: 10, onnotice: () => undefined });
  return drizzle(client, { schema });
}

export function db(): Promise<Db> {
  globalStash.__erwebDb ??= connect();
  return globalStash.__erwebDb;
}

export { schema };
