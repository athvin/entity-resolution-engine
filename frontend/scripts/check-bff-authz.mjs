/**
 * The BFF authorization gate: every route handler that serves tenant or admin
 * data must authorize, in its own file.
 *
 * This is the property the whole tenant-isolation model rests on. `src/proxy.ts`
 * is a cookie-PRESENCE gate only — it runs where there is no database access, so
 * it cannot tell whether the session may see this org (see its own comment).
 * Authorization therefore happens in the route handler, via
 * `requireOrgAccess(org, minimum)` for tenant routes and `requireSuperAdmin()`
 * for the operator console. A handler that forgets is reachable by any
 * authenticated user, with the BFF attaching a privileged credential upstream.
 *
 * At the time of writing all 47 tenant routes and all 7 admin routes comply, so
 * this gate codifies an invariant that already holds rather than asking for new
 * work. It is deliberately a file-level textual check and not a type-aware one:
 * the question "did this file remember to authorize at all" does not need
 * dataflow, and a cheap check that cannot silently stop working is worth more
 * here than a clever one. (It replaced a semgrep rule that reported zero
 * findings because its pattern failed to parse.)
 *
 * Run locally with `pnpm authz:check`. Exported for the unit test that proves it
 * fails on an unauthorized handler.
 */
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

/** Route-handler exports Next treats as HTTP methods. */
const METHOD_EXPORT =
  /export\s+(?:async\s+)?(?:function|const)\s+(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b/;

export const RULES = [
  {
    dir: path.join("src", "app", "api", "orgs"),
    required: "requireOrgAccess",
    hint:
      "Call requireOrgAccess(org, <minimum role>) inside respond() before forwarding. " +
      "It resolves membership, enforces the role rank, confines impersonation to its " +
      "own org, and 404s non-members so they learn nothing (src/lib/bff/proxy.ts).",
  },
  {
    dir: path.join("src", "app", "api", "admin"),
    required: "requireSuperAdmin",
    hint:
      "Call requireSuperAdmin() first. It throws 404 rather than 403 on purpose — a " +
      "non-operator must not learn the route exists (src/lib/bff/admin.ts).",
  },
];

function walk(dir) {
  const out = [];
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else if (entry.name === "route.ts" || entry.name === "route.tsx") out.push(full);
  }
  return out;
}

/**
 * @param {string} root directory containing `src/`
 * @returns {{file: string, required: string, hint: string, methods: string[]}[]} violations
 */
export function findUnauthorizedRoutes(root) {
  const violations = [];
  for (const rule of RULES) {
    for (const file of walk(path.join(root, rule.dir))) {
      const source = readFileSync(file, "utf8");
      const methods = [...source.matchAll(new RegExp(METHOD_EXPORT, "g"))].map((m) => m[1]);
      if (!methods.length) continue; // a route file exporting no handler serves nothing
      if (source.includes(rule.required)) continue;
      violations.push({
        file: path.relative(root, file),
        required: rule.required,
        hint: rule.hint,
        methods: [...new Set(methods)],
      });
    }
  }
  return violations;
}

/** Count of handler files the gate examined — reported so a glob that silently
 * stops matching (a directory rename) shows up as a suspicious zero. */
export function countRoutes(root) {
  return RULES.reduce((total, rule) => total + walk(path.join(root, rule.dir)).length, 0);
}

// Entry point only when executed, so the unit test can import the functions.
if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
  const examined = countRoutes(root);
  const violations = findUnauthorizedRoutes(root);

  if (!examined) {
    console.error(
      "authz gate: examined 0 route files — the api/orgs or api/admin path must have moved.\n" +
        "Update RULES in scripts/check-bff-authz.mjs; do not let this pass as green.",
    );
    process.exit(2);
  }

  if (!violations.length) {
    console.log(`authz gate passed: ${examined} route handlers, all authorize.`);
    process.exit(0);
  }

  console.error(`\nauthz gate FAILED — ${violations.length} route handler(s) never authorize:\n`);
  for (const violation of violations) {
    console.error(`  ${violation.file}  (exports ${violation.methods.join(", ")})`);
    console.error(`    missing: ${violation.required}`);
    console.error(`    ${violation.hint}\n`);
  }
  process.exit(1);
}
