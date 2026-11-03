#!/usr/bin/env node
/**
 * One path to erserver.
 *
 * Every request the BFF makes to erserver must go through
 * `src/lib/bff/upstream.ts`. Before that function existed, five call sites each
 * built the URL and the header set by hand and had already drifted — four set
 * `cache: "no-store"` and one did not — plus a dead sixth
 * (`lib/api/erserver.ts`) that no longer exists. The telemetry work needs
 * `traceparent` on *every* upstream call, and with five paths "every" is
 * something you hope for rather than something you can check.
 *
 * The invariant is expressed as: **only `upstream.ts` may name
 * `ERSERVER_BASE_URL`.** That is the one thing a new path cannot avoid doing —
 * you cannot reach erserver without its base URL — which makes it a far better
 * tripwire than looking for `fetch(`, of which the app has many legitimate uses.
 *
 * ## Deliberately not semgrep
 *
 * `.semgrep.yml`'s own header records why: this class of invariant was tried
 * there as a `pattern-not` and semgrep *rejected the pattern as invalid
 * TypeScript while still reporting the rule as run with zero findings* — a
 * silent false green over the single most important property in the app. A
 * plain script that prints what it examined cannot do that.
 *
 * ## The four properties this copies from check-bff-authz.mjs
 *
 * 1. Walk all of `src/`, not a hand-listed subtree. A directory the gate never
 *    examined is the false-green shape this repo has been bitten by repeatedly.
 * 2. The allowlist is keyed by path and carries a **stated reason**.
 * 3. Every allowlisted caller must actually contain `upstreamFetch(` — so
 *    deleting or renaming a caller fails here instead of rotting silently.
 * 4. Print the examined count, so a moved directory shows up as a suspicious
 *    zero rather than as success.
 */

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

/** The only module allowed to name the upstream base URL. */
const UPSTREAM_MODULE = path.join("src", "lib", "bff", "upstream.ts");

/**
 * Files permitted to name `ERSERVER_BASE_URL`, with the reason each is safe.
 *
 * Adding an entry here is a deliberate act with a justification attached,
 * which is the point: a sixth path should be hard to add by accident and
 * obvious in review when added on purpose.
 */
const BASE_URL_ALLOWED = {
  [UPSTREAM_MODULE]: "the one place a request to erserver is built",
  [path.join("src", "lib", "env.ts")]: "declares and validates the variable itself",
};

/**
 * Every module expected to call `upstreamFetch`, with what it is for.
 *
 * These are checked to CONTAIN the call. The three credential stories differ
 * genuinely — a stored org key by role, the operator token, and the widget's
 * signed token — which is why `authorization` is a parameter rather than
 * resolved inside `upstreamFetch`, and why these remain separate callers
 * instead of being collapsed further.
 */
const EXPECTED_CALLERS = {
  [path.join("src", "lib", "bff", "proxy.ts")]: "the tenant proxy: a stored org key, by role",
  [path.join("src", "lib", "bff", "admin.ts")]: "the operator console: the operator token",
  [path.join("src", "lib", "bff", "relay.ts")]: "the email relay: the platform on its own behalf",
  [path.join("src", "app", "api", "orgs", "[org]", "datasets", "[ds]", "imports", "route.ts")]:
    "multipart upload: passes a FormData body through so the boundary is fetch's to set",
  [path.join("src", "app", "api", "widget", "[token]", "route.ts")]:
    "the embeddable widget: a signed record-scoped token is the credential, no acting person",
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
    if (entry.isDirectory()) {
      if (entry.name === "node_modules" || entry.name.startsWith(".")) continue;
      out.push(...walk(full));
    } else if (/\.(ts|tsx|mts|cts)$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

function main() {
  const root = path.resolve(import.meta.dirname, "..");
  const files = walk(path.join(root, "src"));

  if (files.length === 0) {
    console.error(
      "upstream gate: examined 0 source files — src/ must have moved.\n" +
        "Fix the walk in scripts/check-bff-upstream.mjs; do not let this pass as green.",
    );
    process.exit(2);
  }

  const offenders = [];
  for (const file of files) {
    const rel = path.relative(root, file);
    if (rel in BASE_URL_ALLOWED) continue;
    if (readFileSync(file, "utf8").includes("ERSERVER_BASE_URL")) offenders.push(rel);
  }

  const missing = [];
  for (const [rel, reason] of Object.entries(EXPECTED_CALLERS)) {
    let source;
    try {
      source = readFileSync(path.join(root, rel), "utf8");
    } catch {
      missing.push(`${rel} — not found (${reason})`);
      continue;
    }
    if (!source.includes("upstreamFetch(")) {
      missing.push(`${rel} — does not call upstreamFetch( (${reason})`);
    }
  }

  if (offenders.length > 0) {
    console.error(
      `\nupstream gate FAILED — ${String(offenders.length)} file(s) name ERSERVER_BASE_URL ` +
        `outside ${UPSTREAM_MODULE}:\n`,
    );
    for (const file of offenders) console.error(`  ${file}`);
    console.error(
      "\nEvery request to erserver goes through upstreamFetch in src/lib/bff/upstream.ts,\n" +
        "so that anything which must be on every upstream call — traceparent, the acting\n" +
        "user, the no-store policy — is attached in one place rather than in five. If this\n" +
        "really is a legitimate new path, add it to BASE_URL_ALLOWED with the reason.\n",
    );
    process.exit(1);
  }

  if (missing.length > 0) {
    console.error(`\nupstream gate FAILED — ${String(missing.length)} expected caller(s) wrong:\n`);
    for (const line of missing) console.error(`  ${line}`);
    console.error(
      "\nAn entry in EXPECTED_CALLERS that no longer calls upstreamFetch is either a path\n" +
        "that regressed to building its own request, or a stale entry. Fix the caller, or\n" +
        "remove the entry in the same commit that removes the caller.\n",
    );
    process.exit(1);
  }

  console.log(
    `upstream gate passed: ${String(files.length)} source files examined, ` +
      `${String(Object.keys(EXPECTED_CALLERS).length)} callers all route through upstreamFetch.`,
  );
}

main();
