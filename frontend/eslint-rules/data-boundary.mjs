/**
 * The data boundary: the frontend reaches data through er-server, never around it.
 *
 * Three tiers, three rules:
 *
 * 1. **Nothing in `src/` opens the lake or object storage.** DuckDB, DuckLake,
 *    MotherDuck, Parquet readers and AWS SDK clients are banned outright. A
 *    tenant's lake is isolated by catalog separation that er-server enforces
 *    (see the security skill); a DuckDB-WASM import in a client component would
 *    build, run, and go straight past it. The frontend has no lake credential
 *    and must never grow one — if a screen needs lake data, er-server serves it
 *    (the Arrow read surface exists for exactly that).
 *
 * 2. **Exactly one module opens Postgres.** The BFF owns its own `erweb` schema
 *    (sessions, memberships, the sealed key vault) and talks to it through
 *    drizzle. The connection itself — `postgres`, PGlite, any drizzle driver
 *    subpath — may only be imported under `src/lib/db/`, which is
 *    `server-only`. Route handlers use the handle `db()` returns and the
 *    driver-agnostic `drizzle-orm` operators; they never construct a client.
 *
 * 3. **A `"use client"` module is browser code.** It may not import the
 *    database layer, `drizzle-orm`, the privileged upstream client, the BFF
 *    helpers or the env reader, and it may not call `fetch`, `EventSource`,
 *    `WebSocket` or `sendBeacon` with an absolute URL. The browser talks to
 *    the BFF, same-origin, and nothing else. (`server-only` already turns a
 *    transitive import of those modules into a build error; this rule names
 *    the direct case at lint time, and covers `drizzle-orm`, which is not
 *    `server-only` itself. CSP `connect-src 'self'` is the runtime layer.)
 *
 * Type-only imports are allowed everywhere: they are erased, so they cannot
 * open a connection or ship code to the browser.
 */
import path from "node:path";

/** Rule 1: never imported anywhere under src/. */
export const LAKE_AND_FOREIGN_STORES = [
  /^duckdb(-async|-lambda)?(\/|$)/,
  /^@duckdb\//,
  /^@motherduck\//,
  /^@aws-sdk\//,
  /^aws-sdk(\/|$)/,
  /^hyparquet(\/|$|-)/,
  /^parquet-wasm(\/|$)/,
  /^parquetjs(-lite)?(\/|$)/,
  /^@dsnp\/parquetjs(\/|$)/,
  /^pg(\/|$)/,
  /^pg-/,
  /^@neondatabase\//,
  /^@vercel\/postgres(\/|$)/,
  /^drizzle-orm\/duckdb/,
];

/** Rule 2: the Postgres connection — only under src/lib/db/. */
export const POSTGRES_DRIVERS = [
  /^postgres(\/|$)/,
  /^@electric-sql\/pglite(\/|$)/,
  // Every drizzle subpath except the dialect-level query builder is a driver
  // (postgres-js, pglite, node-postgres, neon-http, …) or a driver's migrator.
  /^drizzle-orm\/(?!pg-core(\/|$))/,
];

export const DRIVER_DIR = "src/lib/db/";

/** Rule 3: server modules a "use client" file may not import. */
export const SERVER_MODULES = [
  /^drizzle-orm(\/|$)/,
  /^@\/lib\/db(\/|$)/,
  /^@\/lib\/api\/erserver$/,
  // …except the error envelope, which is the one BFF module both sides share on
  // purpose: the browser client throws `BffRequestError` over the same shape.
  /^@\/lib\/bff(?!\/errors$)(\/|$)/,
  /^@\/lib\/env$/,
];

const ABSOLUTE_URL = /^(https?:|wss?:|\/\/)/i;
const NETWORK_CALLS = new Set(["fetch", "sendBeacon"]);
const NETWORK_CTORS = new Set(["EventSource", "WebSocket"]);

/** Forward-slash path of `filename` relative to the nearest `src/` root, or null. */
function srcRelative(filename) {
  const normalized = filename.split(path.sep).join("/");
  const idx = normalized.lastIndexOf("/src/");
  if (idx === -1) return normalized.startsWith("src/") ? normalized : null;
  return normalized.slice(idx + 1);
}

/** A relative specifier rewritten into its `@/…` alias form, so one list covers both. */
function canonical(specifier, filename) {
  if (!specifier.startsWith(".")) return specifier;
  const from = srcRelative(filename);
  if (from === null) return specifier;
  const resolved = path.posix.normalize(path.posix.join(path.posix.dirname(from), specifier));
  return resolved.startsWith("src/")
    ? `@/${resolved.slice(4).replace(/\.(m?[jt]sx?)$/, "")}`
    : specifier;
}

function isUseClient(program) {
  for (const statement of program.body) {
    if (statement.type !== "ExpressionStatement" || typeof statement.directive !== "string") break;
    if (statement.directive === "use client") return true;
  }
  return false;
}

function isTypeOnly(node) {
  if (node.importKind === "type" || node.exportKind === "type") return true;
  const specifiers = node.specifiers ?? [];
  return (
    specifiers.length > 0 &&
    specifiers.every((s) => s.importKind === "type" || s.exportKind === "type")
  );
}

function literalPrefix(node) {
  if (!node) return null;
  if (node.type === "Literal" && typeof node.value === "string") return node.value;
  if (node.type === "TemplateLiteral") return node.quasis[0]?.value.cooked ?? null;
  return null;
}

/** `fetch`, `window.fetch`, `globalThis.fetch`, `navigator.sendBeacon`, … */
function calleeName(callee) {
  if (callee.type === "Identifier") return callee.name;
  if (
    callee.type === "MemberExpression" &&
    !callee.computed &&
    callee.property.type === "Identifier"
  )
    return callee.property.name;
  return null;
}

/** @type {import("eslint").Rule.RuleModule} */
export const dataBoundary = {
  meta: {
    type: "problem",
    docs: {
      description:
        "The frontend reaches data only through er-server: no lake or object-store client anywhere, Postgres only from src/lib/db, and browser code only same-origin.",
    },
    schema: [],
    messages: {
      lake: "'{{source}}' opens a data store directly. The frontend never touches DuckDB, DuckLake, S3 or a foreign database — serve the data from er-server and call it through src/lib/api/erserver.ts in a route handler.",
      driver:
        "'{{source}}' constructs a Postgres connection. Only src/lib/db/ may do that; use the handle from `db()` in '@/lib/db' instead.",
      clientImport:
        "'{{source}}' is server code and this is a \"use client\" module. Browser code reaches data through the BFF: a TanStack Query hook in src/lib/query/ calling a route handler via src/lib/api/client.ts.",
      clientUrl:
        "Browser code may only call the BFF, same-origin. Use a relative '/api/…' path; if the data lives elsewhere, a route handler fetches it server-side.",
    },
  },
  create(context) {
    const filename = context.filename;
    const rel = srcRelative(filename);
    if (rel === null) return {};
    const inDriverDir = rel.startsWith(DRIVER_DIR);
    let browser = false;

    function check(node, sourceNode, typeOnly) {
      const source = literalPrefix(sourceNode);
      if (source === null || typeOnly) return;
      if (LAKE_AND_FOREIGN_STORES.some((re) => re.test(source))) {
        context.report({ node, messageId: "lake", data: { source } });
        return;
      }
      if (!inDriverDir && POSTGRES_DRIVERS.some((re) => re.test(source))) {
        context.report({ node, messageId: "driver", data: { source } });
        return;
      }
      if (browser && SERVER_MODULES.some((re) => re.test(canonical(source, filename)))) {
        context.report({ node, messageId: "clientImport", data: { source } });
      }
    }

    function checkUrl(node, arg) {
      if (!browser) return;
      const url = literalPrefix(arg);
      if (url !== null && ABSOLUTE_URL.test(url)) context.report({ node, messageId: "clientUrl" });
    }

    return {
      Program(node) {
        browser = isUseClient(node);
      },
      ImportDeclaration(node) {
        check(node, node.source, isTypeOnly(node));
      },
      ExportNamedDeclaration(node) {
        if (node.source) check(node, node.source, isTypeOnly(node));
      },
      ExportAllDeclaration(node) {
        check(node, node.source, node.exportKind === "type");
      },
      ImportExpression(node) {
        check(node, node.source, false);
      },
      CallExpression(node) {
        const name = calleeName(node.callee);
        if (name === "require" && node.callee.type === "Identifier")
          check(node, node.arguments[0], false);
        else if (name !== null && NETWORK_CALLS.has(name)) checkUrl(node, node.arguments[0]);
      },
      NewExpression(node) {
        const name = calleeName(node.callee);
        if (name !== null && NETWORK_CTORS.has(name)) checkUrl(node, node.arguments[0]);
      },
    };
  },
};

export default {
  meta: { name: "er-data-boundary" },
  rules: { "data-boundary": dataBoundary },
};
