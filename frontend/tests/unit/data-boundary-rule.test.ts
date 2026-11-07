import { RuleTester } from "eslint";
import tseslint from "typescript-eslint";
import { describe, it } from "vitest";

import { dataBoundary } from "../../eslint-rules/data-boundary.mjs";

/**
 * eslint-rules/data-boundary.mjs keeps the frontend from reaching a data store
 * around er-server: no DuckDB / DuckLake / S3 / foreign-database client anywhere
 * in src/, the Postgres connection only from src/lib/db/, and browser code only
 * same-origin. Every invalid case below plants a violation the rule must
 * report — a gate nobody has watched fail is not a gate (.claude/skills/frontend-testing/references/frontend-testing.md).
 */

RuleTester.describe = describe;
RuleTester.it = it;
RuleTester.itOnly = it.only;

const tester = new RuleTester({
  languageOptions: { parser: tseslint.parser, ecmaVersion: 2024, sourceType: "module" },
});

const ROUTE = "src/app/api/orgs/[org]/things/route.ts";
const CLIENT = "src/components/things/things-list.tsx";
const DB = "src/lib/db/index.ts";

const client = (body: string) => `"use client";\n${body}`;

tester.run("data-boundary", dataBoundary, {
  valid: [
    // Route handlers query erweb through the shared handle and the
    // driver-agnostic operators.
    {
      filename: ROUTE,
      code: `import { eq } from "drizzle-orm";\nimport { db, schema } from "@/lib/db";`,
    },
    // …and call er-server server-side, absolute URL and all.
    { filename: ROUTE, code: `await fetch("http://localhost:8000/v1/orgs");` },
    // The one module that opens the connection.
    {
      filename: DB,
      code: `const { default: postgres } = await import("postgres");\nconst { PGlite } = await import("@electric-sql/pglite");\nconst { drizzle } = await import("drizzle-orm/postgres-js");`,
    },
    { filename: "src/lib/db/schema.ts", code: `import { pgSchema } from "drizzle-orm/pg-core";` },
    // Browser code calling the BFF same-origin.
    { filename: CLIENT, code: client(`await fetch("/api/orgs/acme/things");`) },
    { filename: CLIENT, code: client("await fetch(`/api/orgs/${org}/things`);") },
    { filename: CLIENT, code: client(`import { bffFetch } from "@/lib/api/client";`) },
    // The shared error envelope, and type-only imports, are erased or shared.
    { filename: CLIENT, code: client(`import { BFF_ERROR_CODES } from "@/lib/bff/errors";`) },
    { filename: CLIENT, code: client(`import type { Db } from "@/lib/db";`) },
    { filename: CLIENT, code: client(`import { type Identity } from "@/lib/bff/admin";`) },
    // Outside src/ (the migrate script) is not the app.
    { filename: "scripts/db-migrate.mjs", code: `import postgres from "postgres";` },
  ],
  invalid: [
    // Rule 1 — the lake and object storage, anywhere, even under src/lib/db.
    {
      filename: CLIENT,
      code: client(`import * as duckdb from "@duckdb/duckdb-wasm";`),
      errors: [{ messageId: "lake" }],
    },
    {
      filename: ROUTE,
      code: `import { S3Client } from "@aws-sdk/client-s3";`,
      errors: [{ messageId: "lake" }],
    },
    { filename: DB, code: `import duckdb from "duckdb";`, errors: [{ messageId: "lake" }] },
    {
      filename: ROUTE,
      code: `const d = require("duckdb-async");`,
      errors: [{ messageId: "lake" }],
    },
    { filename: ROUTE, code: `export * from "pg";`, errors: [{ messageId: "lake" }] },
    {
      filename: ROUTE,
      code: `const { parquetRead } = await import("hyparquet");`,
      errors: [{ messageId: "lake" }],
    },
    // Rule 2 — a Postgres connection constructed outside src/lib/db.
    {
      filename: ROUTE,
      code: `import postgres from "postgres";`,
      errors: [{ messageId: "driver" }],
    },
    {
      filename: "src/lib/usage/record.ts",
      code: `import { drizzle } from "drizzle-orm/postgres-js";`,
      errors: [{ messageId: "driver" }],
    },
    {
      filename: ROUTE,
      code: `const { PGlite } = await import("@electric-sql/pglite");`,
      errors: [{ messageId: "driver" }],
    },
    // Rule 3 — browser code importing server code…
    {
      filename: CLIENT,
      code: client(`import { db } from "@/lib/db";`),
      errors: [{ messageId: "clientImport" }],
    },
    {
      filename: CLIENT,
      code: client(`import { eq } from "drizzle-orm";`),
      errors: [{ messageId: "clientImport" }],
    },
    {
      filename: CLIENT,
      code: client(`import { erserver } from "../../lib/api/erserver";`),
      errors: [{ messageId: "clientImport" }],
    },
    {
      filename: CLIENT,
      code: client(`import { requireOrgAccess } from "@/lib/bff/proxy";`),
      errors: [{ messageId: "clientImport" }],
    },
    {
      filename: CLIENT,
      code: client(`import { env } from "@/lib/env";`),
      errors: [{ messageId: "clientImport" }],
    },
    // …or calling anything but the same-origin BFF.
    {
      filename: CLIENT,
      code: client(`await fetch("http://er-server:8000/v1/orgs");`),
      errors: [{ messageId: "clientUrl" }],
    },
    {
      filename: CLIENT,
      code: client("await window.fetch(`https://lake.example/${key}`);"),
      errors: [{ messageId: "clientUrl" }],
    },
    {
      filename: CLIENT,
      code: client(`const ws = new WebSocket("wss://db.example/stream");`),
      errors: [{ messageId: "clientUrl" }],
    },
    {
      filename: CLIENT,
      code: client(`navigator.sendBeacon("//collector.example/b", body);`),
      errors: [{ messageId: "clientUrl" }],
    },
  ],
});
