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
    // EITHER guard: requireDatasetAccess IS requireOrgAccess plus the dataset
    // slug check, so a dataset-scoped handler under orgs/[org]/datasets/[ds]
    // satisfies the same invariant through it.
    required: ["requireOrgAccess", "requireDatasetAccess"],
    hint:
      "Call requireOrgAccess(org, <minimum role>) — or, under datasets/[ds], " +
      "requireDatasetAccess(org, ds, <minimum role>) — inside respond() before " +
      "forwarding. It resolves membership, enforces the role rank, confines " +
      "impersonation to its own org, and 404s non-members so they learn nothing " +
      "(src/lib/bff/proxy.ts).",
  },
  {
    dir: path.join("src", "app", "api", "admin"),
    required: "requireSuperAdmin",
    hint:
      "Call requireSuperAdmin() first. It throws 404 rather than 403 on purpose — a " +
      "non-operator must not learn the route exists (src/lib/bff/admin.ts).",
  },
  {
    dir: path.join("src", "app", "api", "impersonation"),
    // EITHER guard. Starting an impersonation is a super-admin act; ENDING one
    // needs only that you are the person inside it, so a plain identity check
    // is correct there and demanding super admin would strand a session whose
    // operator rights were revoked mid-impersonation.
    required: ["requireSuperAdmin", "requireIdentity"],
    hint:
      "Starting an impersonation needs requireSuperAdmin(); ending one needs " +
      "requireIdentity(). This directory was invisible to the gate until the " +
      "uncovered-directory check was added.",
  },
];

/**
 * API directories that deliberately do NOT authorize through a session, each
 * with the reason it is safe.
 *
 * This list exists because the gate used to scan only `orgs` and `admin`: a new
 * top-level API directory was simply never examined, and the gate reported
 * success having not looked at it. That is the false-green shape this repo has
 * been bitten by repeatedly, so an UNKNOWN directory is now a failure and
 * adding one is a deliberate entry here rather than a silent omission.
 */
const UNAUTHENTICATED_DIRS = {
  auth: "login, logout and password reset — how a session is obtained in the first place",
  invites: "the token in the URL is the credential; a session does not exist yet",
  "csp-report": "the browser posts these with no credentials, and the pre-login ones matter most",
  widget: "O11's embeddable widget: a signed, record-scoped, expiring token is the credential",
  healthz:
    "the container's own readiness probe — a kubelet carries no session, and the body is a boolean plus a reason class",
};

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
      const accepted = Array.isArray(rule.required) ? rule.required : [rule.required];
      if (accepted.some((guard) => source.includes(guard))) continue;
      violations.push({
        file: path.relative(root, file),
        required: Array.isArray(rule.required) ? rule.required.join(" or ") : rule.required,
        hint: rule.hint,
        methods: [...new Set(methods)],
      });
    }
  }
  return violations;
}

/**
 * API directories the gate neither checks nor knowingly exempts.
 *
 * A non-empty result is a gate hole, not a style issue: the directory serves
 * HTTP and nothing has asked whether it authorizes.
 */
export function findUncoveredApiDirs(root) {
  const apiRoot = path.join(root, "src", "app", "api");
  const ruled = new Set(RULES.map((rule) => path.basename(rule.dir)));
  let entries;
  try {
    entries = readdirSync(apiRoot, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .filter((name) => !ruled.has(name) && !(name in UNAUTHENTICATED_DIRS));
}

/** Count of handler files the gate examined — reported so a glob that silently
 * stops matching (a directory rename) shows up as a suspicious zero. */
export function countRoutes(root) {
  return RULES.reduce((total, rule) => total + walk(path.join(root, rule.dir)).length, 0);
}

/**
 * Route files that name the attempt channel's path prefix.
 *
 * The `/v1/internal/*` routes are machine-to-machine (the task pod reporting
 * over the attempt channel, batch-orchestration-design §6.4): they are
 * reachable only in-namespace, and the BFF must NEVER forward them — a BFF
 * route naming that prefix would hand a browser session a path to the
 * pod-credentialed surface. Unlike the authorization rules above this scans
 * EVERY route file under src/app/api, because the property is "nowhere", not
 * "somewhere with a guard".
 */
export function findInternalForwards(root) {
  const offenders = [];
  for (const file of walk(path.join(root, "src", "app", "api"))) {
    if (readFileSync(file, "utf8").includes("/v1/internal")) {
      offenders.push(path.relative(root, file));
    }
  }
  return offenders;
}

// Entry point only when executed, so the unit test can import the functions.
if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
  const examined = countRoutes(root);
  const violations = findUnauthorizedRoutes(root);
  const uncovered = findUncoveredApiDirs(root);
  const internal = findInternalForwards(root);

  if (internal.length) {
    console.error(`\nauthz gate FAILED — ${internal.length} route file(s) name /v1/internal:\n`);
    for (const file of internal) console.error(`  ${file}`);
    console.error(
      "\nThe attempt channel (/v1/internal/*) is the task pod's machine-to-machine\n" +
        "surface; the BFF never forwards it (batch-orchestration-design §6.4). There is\n" +
        "no exemption list on purpose — the property is 'nowhere'.\n",
    );
    process.exit(1);
  }

  if (uncovered.length) {
    console.error(
      `\nauthz gate FAILED — ${uncovered.length} api director(ies) this gate does not cover:\n`,
    );
    for (const name of uncovered) console.error(`  src/app/api/${name}`);
    console.error(
      "\nEither add a RULE (if it serves tenant or admin data through a session) or an\n" +
        "UNAUTHENTICATED_DIRS entry with the reason it is safe. A directory the gate has\n" +
        "never looked at reports green while serving HTTP, which is how this check\n" +
        "silently stops covering new surface.\n",
    );
    process.exit(1);
  }

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
