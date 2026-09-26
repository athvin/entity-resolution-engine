/**
 * Apply the committed SQL migrations in ./drizzle to ERWEB_DATABASE_URL.
 * Deliberately tiny: drizzle-orm's migrator over postgres.js, nothing else.
 */
import { drizzle } from "drizzle-orm/postgres-js";
import { migrate } from "drizzle-orm/postgres-js/migrator";
import postgres from "postgres";

const url = process.env.ERWEB_DATABASE_URL;
if (!url) {
  console.error("ERWEB_DATABASE_URL is required");
  process.exit(2);
}

const client = postgres(url, { max: 1, onnotice: () => {} });
try {
  await migrate(drizzle(client), {
    migrationsFolder: new URL("../drizzle", import.meta.url).pathname,
  });
  console.log("erweb migrations applied");
} finally {
  await client.end();
}
