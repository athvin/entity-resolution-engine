/**
 * The BFF envelope gate: a route may not misread the shape er-server sends.
 *
 * This is the half the wire-contract and mock-fabrication gates both record as
 * out of reach. They compare *field names*; `mock_contract.rs` compares route
 * and method. None of them sees W0.5b: the jobs route declared
 * `forward<unknown[]>` over a body er-server sends as `{jobs: […]}` — a type
 * *assertion* about an HTTP body TypeScript cannot check — so the runs page's
 * table rendered nothing against the real backend while every mock-backed tier
 * was green.
 *
 * The authority is `src/lib/api/envelopes.json`, generated from the handlers by
 * `er-server/tests/response_envelopes.rs` and byte-compared there. It is
 * derived, never declared: a hand-maintained table of response shapes — or a
 * `#[utoipa::path]` response body — would be one more claim free to drift from
 * the code, which is the defect class itself.
 *
 * **There is no convention to infer, which is why this gate is needed.** Across
 * 43 routed GETs the ledger finds six shapes: 10 `bare`, 10 `wrapped`, 10
 * `record`, 10 `scalar`, 2 `page`, 1 `arrow`. `/runs`, `/webhooks` and `/audit`
 * are bare; `/jobs`, `/webhooks/deliveries` and `/segments` wrap. Within one
 * resource family both spellings occur, so a developer reading two neighbouring
 * routes learns the wrong rule either way.
 *
 * ## The failure mode `listFrom` introduced
 *
 * `listFrom(body, key)` returns `[]` when `key` is absent, deliberately, so a
 * list endpoint answering `null` cannot crash a `.map()` three components deep.
 * The cost is that unwrapping the **wrong** key is silent: an empty table, no
 * error, nothing in a log. So this gate checks the key against the ledger's
 * actual keys — that specific check is the reason it exists at all, because it
 * is the one mistake the tolerant helper cannot surface at runtime.
 *
 * ## Two things this had to get right to be worth anything
 *
 * Both were false positives in the first draft, and both are the same mistake
 * in different clothes — reading source without reading it properly:
 *
 * - **Comments are stripped before matching.** The jobs route's comment quotes
 *   the historical `forward<unknown[]>` bug verbatim, so a naive scan reports
 *   the very defect the comment is explaining was fixed.
 * - **Path parameter names are erased on both sides.** The BFF spells it
 *   `${jobId}` and the router spells it `{job_id}`; comparing them literally
 *   leaves every detail route unmatched, which reads as "not a routed GET".
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

const REPO = path.resolve(__dirname, "../../..");
const API_DIR = path.join(REPO, "frontend/src/app/api");
const LEDGER_PATH = path.join(REPO, "frontend/src/lib/api/envelopes.json");

interface LedgerEntry {
  envelope: "bare" | "wrapped" | "record" | "scalar" | "page" | "arrow";
  key?: string;
  keys?: string[];
  cursor_in?: string;
}

/** Shapes that carry the payload under a key, so unwrapping is required. */
const KEYED = new Set(["wrapped", "record", "page"]);

function ledger(): Record<string, LedgerEntry> {
  return JSON.parse(readFileSync(LEDGER_PATH, "utf8")) as Record<string, LedgerEntry>;
}

/** The keys a ledger entry actually carries. */
function keysOf(entry: LedgerEntry): string[] {
  if (entry.keys) return entry.keys;
  return entry.key ? [entry.key] : [];
}

/**
 * Remove comments while leaving string and template literals intact. A regex
 * for `//` cannot do this: a `//` inside a URL string would eat the rest of the
 * line, and a comment quoting code would otherwise be matched as code.
 */
export function stripComments(src: string): string {
  let out = "";
  let i = 0;
  type Mode = "code" | "line" | "block" | "single" | "double" | "template";
  let mode: Mode = "code";
  while (i < src.length) {
    const two = src.slice(i, i + 2);
    if (mode === "code") {
      if (two === "//") {
        mode = "line";
        i += 2;
        continue;
      }
      if (two === "/*") {
        mode = "block";
        i += 2;
        continue;
      }
      if (src.charAt(i) === "'") mode = "single";
      else if (src.charAt(i) === '"') mode = "double";
      else if (src.charAt(i) === "`") mode = "template";
      out += src.charAt(i);
      i += 1;
      continue;
    }
    if (mode === "line") {
      if (src.charAt(i) === "\n") {
        mode = "code";
        out += "\n";
      }
      i += 1;
      continue;
    }
    if (mode === "block") {
      if (two === "*/") {
        mode = "code";
        i += 2;
      } else {
        i += 1;
      }
      continue;
    }
    // inside a literal: copy through, honouring escapes
    if (src.charAt(i) === "\\") {
      out += src.slice(i, i + 2);
      i += 2;
      continue;
    }
    if (
      (mode === "single" && src.charAt(i) === "'") ||
      (mode === "double" && src.charAt(i) === '"') ||
      (mode === "template" && src.charAt(i) === "`")
    ) {
      mode = "code";
    }
    out += src.charAt(i);
    i += 1;
  }
  return out;
}

/** `/v1/orgs/${org}/jobs/${jobId}` and `/v1/orgs/{org}/jobs/{job_id}` -> same. */
export function erasePathParams(p: string): string {
  return p.replace(/\$\{[^}]*\}/g, "{}").replace(/\{[^}]*\}/g, "{}");
}

/**
 * The BFF spells sub-path actions with a colon — `assertions:contradictions`,
 * `jobs/${id}:resume` — inherited from the Python API, and er-server accepts it
 * through `rewrite_action_path`, an outer service that normalizes the URI
 * before routing (it is `pub` precisely so `mock_contract.rs` can apply the
 * same rule). The router itself only ever registers the `/` spelling, so a
 * lookup against the ledger has to normalize first or every colon route reads
 * as unrouted.
 *
 * This duplicates one small rule across languages, knowingly: the alternative
 * is teaching the generator to emit both spellings per route, which puts an
 * alias in the ledger that no handler actually registers. A colon that is
 * percent-encoded (`crm%3AC001`, an id rather than an action) is left alone by
 * the Rust side and by this, since `%3A` is not a literal `:`.
 */
export function normalizeActionPath(p: string): string {
  const at = p.lastIndexOf(":");
  if (at === -1) return p;
  // Only a colon in the final segment names an action.
  if (p.slice(at).includes("/")) return p;
  return `${p.slice(0, at)}/${p.slice(at + 1)}`;
}

function routeFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) out.push(...routeFiles(full));
    else if (name === "route.ts") out.push(full);
  }
  return out.sort();
}

interface Expectation {
  bff: string;
  upstream: string;
  /** How the route reads the body. */
  reads: "bare-array" | "unwrap" | "opaque";
  /** The key it unwraps, when `reads === "unwrap"`. */
  key?: string;
}

/** What each BFF GET route expects of its upstream response. */
function expectations(): Expectation[] {
  const out: Expectation[] = [];
  for (const file of routeFiles(API_DIR)) {
    const src = stripComments(readFileSync(file, "utf8"));
    const getAt = src.search(/export async function GET\b/);
    if (getAt === -1) continue;
    let body = src.slice(getAt);
    const next = body.slice(1).search(/\nexport async function (?!GET)/);
    if (next !== -1) body = body.slice(0, next + 1);
    const upstream = /`(\/v1\/[^`]*)`/.exec(body);
    if (!upstream) continue;
    const rawPath = upstream[1];
    if (rawPath === undefined) continue;
    const unwrap = /listFrom\(\s*[A-Za-z_$][\w$]*\s*,\s*"([^"]+)"\s*\)/.exec(body);
    const bare = /forward<\s*(?:unknown|[A-Za-z_$][\w$]*)\s*\[\]\s*>/.test(body);
    const key = unwrap?.[1];
    const stripped = rawPath.split("?")[0] ?? rawPath;
    out.push({
      bff: path.relative(API_DIR, file),
      upstream: erasePathParams(normalizeActionPath(stripped.replace(/\/$/, ""))),
      reads: bare ? "bare-array" : unwrap ? "unwrap" : "opaque",
      // `exactOptionalPropertyTypes` distinguishes an absent property from one
      // set to undefined, so the key is spread in only when there is one.
      ...(key === undefined ? {} : { key }),
    });
  }
  return out;
}

/** The ledger, keyed by erased path. */
function ledgerByErasedPath(): Map<string, { path: string; entry: LedgerEntry }> {
  const out = new Map<string, { path: string; entry: LedgerEntry }>();
  for (const [route, entry] of Object.entries(ledger())) {
    const p = route.replace(/^GET\s+/, "");
    out.set(erasePathParams(p), { path: p, entry });
  }
  return out;
}

describe("the BFF reads the envelope er-server actually sends", () => {
  it("has a generated ledger to check against", () => {
    const l = ledger();
    // A ledger that failed to parse, or shrank to nothing, would make every
    // assertion below pass while checking nothing.
    expect(Object.keys(l).length).toBeGreaterThan(30);
    expect(l["GET /v1/orgs/{org}/jobs"]).toEqual({ envelope: "wrapped", key: "jobs" });
    expect(l["GET /v1/orgs/{org}/runs"]).toEqual({ envelope: "bare" });
  });

  it("finds the BFF GET routes", () => {
    expect(expectations().length).toBeGreaterThan(20);
  });

  it("strips comments without eating string literals", () => {
    // The jobs route's comment quotes `forward<unknown[]>`; without this the
    // gate reports the bug the comment says was fixed.
    expect(stripComments("const a = 1; // forward<unknown[]>\nconst b = 2;")).not.toContain(
      "forward<unknown[]>",
    );
    expect(stripComments('const u = "https://x/y"; // note')).toContain("https://x/y");
    expect(stripComments("/* forward<unknown[]> */ const a = 1;")).not.toContain("forward");
    expect(stripComments("const t = `a//b`;")).toContain("a//b");
  });

  it("normalizes the colon action spelling the BFF uses", () => {
    // `rewrite_action_path` in er-server is the authority; this mirrors it.
    expect(normalizeActionPath("/v1/orgs/{org}/assertions:contradictions")).toBe(
      "/v1/orgs/{org}/assertions/contradictions",
    );
    expect(normalizeActionPath("/v1/orgs/{org}/jobs/${id}:resume")).toBe(
      "/v1/orgs/{org}/jobs/${id}/resume",
    );
    // A colon earlier in the path is not an action.
    expect(normalizeActionPath("/v1/orgs/{org}/runs")).toBe("/v1/orgs/{org}/runs");
    // A percent-encoded colon is an id, not an action.
    expect(normalizeActionPath("/v1/orgs/{org}/records/crm%3AC001")).toBe(
      "/v1/orgs/{org}/records/crm%3AC001",
    );
  });

  it("erases path parameter names on both spellings", () => {
    expect(erasePathParams("/v1/orgs/${org}/jobs/${jobId}")).toBe(
      erasePathParams("/v1/orgs/{org}/jobs/{job_id}"),
    );
  });

  it("every BFF upstream path is a routed GET", () => {
    const known = ledgerByErasedPath();
    const missing = expectations().filter((e) => !known.has(e.upstream));
    expect(
      missing.map((m) => `${m.bff} -> ${m.upstream}`),
      "a BFF route forwards to a path the router does not serve as GET. Either the path is " +
        "misspelled, or the handler is registered under a different method — both are 405s " +
        "against a real backend that no mock-backed tier can see.",
    ).toEqual([]);
  });

  it("never asserts a bare array over a keyed body", () => {
    const known = ledgerByErasedPath();
    const bad: string[] = [];
    for (const e of expectations()) {
      if (e.reads !== "bare-array") continue;
      const found = known.get(e.upstream);
      if (!found || !KEYED.has(found.entry.envelope)) continue;
      bad.push(
        `${e.bff} asserts a bare array over ${e.upstream}, which er-server sends as ` +
          `${found.entry.envelope} (${keysOf(found.entry).join(", ")})`,
      );
    }
    expect(
      bad,
      "this is W0.5b exactly: a `forward<T[]>` is an assertion about an HTTP body, not a check " +
        "of one. Unwrap with `listFrom(body, key)` instead.",
    ).toEqual([]);
  });

  it("unwraps a key the response actually has", () => {
    const known = ledgerByErasedPath();
    const bad: string[] = [];
    for (const e of expectations()) {
      const key = e.key;
      if (e.reads !== "unwrap" || key === undefined) continue;
      const found = known.get(e.upstream);
      if (!found || !KEYED.has(found.entry.envelope)) continue;
      const keys = keysOf(found.entry);
      if (keys.includes(key)) continue;
      bad.push(`${e.bff} unwraps "${key}" from ${e.upstream}, whose keys are [${keys.join(", ")}]`);
    }
    expect(
      bad,
      "`listFrom` returns [] for a missing key by design, so the wrong key is an empty table " +
        "with no error anywhere — the one mistake it cannot surface at runtime.",
    ).toEqual([]);
  });
});
