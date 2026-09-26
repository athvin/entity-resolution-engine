import { defineConfig } from "drizzle-kit";

export default defineConfig({
  schema: "./src/lib/db/schema.ts",
  out: "./drizzle",
  dialect: "postgresql",
  dbCredentials: {
    url: process.env.ERWEB_DATABASE_URL ?? "postgresql://postgres:er@localhost:5433/postgres",
  },
  schemaFilter: ["erweb"],
});
