import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";

/**
 * Detect an ADOPTED server that is serving code older than the working tree.
 *
 * `playwright.config.ts` sets `reuseExistingServer` outside CI, which is what
 * makes the iteration loop fast — and what lets a server left running from an
 * earlier build keep serving the old bundle, or a mock-erserver left running
 * ignore fixture edits. `dev/kill-servers.sh` has always been the remedy. What
 * was missing was DETECTION: a stale server produces confident, wrong results,
 * and the only symptom is tests that disagree with CI for no visible reason.
 *
 * That is not hypothetical. A stray mock left listening by a one-off debugging
 * script was adopted by four consecutive full-tier runs; they reported 3, then
 * 25, then 26, then 27 failures spread across unrelated specs. The spread was
 * read as machine load and the same stale server was then used to take a
 * "baseline" on a clean checkout — which agreed, because both runs adopted it.
 * Two of those runs would have been unnecessary with the check below.
 *
 * Both checks are one-sided on purpose: nothing listening is FINE (the harness
 * is about to start it), and a server the harness itself just started always
 * matches. So this is correct whether Playwright starts `webServer` before or
 * after `globalSetup`.
 */

/** Files whose content the mock serves; an edit to any of them matters. */
const MOCK_DIR = "tests/mock-erserver";

/**
 * The hash the mock reports for itself, over the same inputs
 * [`mockSourceHash`] covers.
 */
export async function fetchMockSourceHash(base: string): Promise<string | null> {
  try {
    const response = await fetch(`${base}/__source`, {
      signal: AbortSignal.timeout(2_000),
    });
    if (!response.ok) return null;
    const body = (await response.json()) as { hash?: unknown };
    return typeof body.hash === "string" ? body.hash : null;
  } catch {
    // Not listening, or listening and not our mock. Either way there is
    // nothing to compare: the harness will start its own.
    return null;
  }
}

/** Every file under `tests/mock-erserver`, sorted, so the hash is stable. */
function mockSourceFiles(root: string): string[] {
  const dir = path.join(root, MOCK_DIR);
  return readdirSync(dir)
    .filter((name) => name.endsWith(".mjs") || name.endsWith(".json"))
    .sort()
    .map((name) => path.join(dir, name));
}

/**
 * A hash of the mock's own sources, as the working tree holds them.
 *
 * Covers the whole directory rather than `server.mjs` alone: the fixtures and
 * the Arrow helper beside it are equally part of what the mock answers with,
 * and an edit to either is equally invisible in an adopted process.
 */
export function mockSourceHash(root: string): string {
  const hash = createHash("sha256");
  for (const file of mockSourceFiles(root)) {
    hash.update(path.basename(file));
    hash.update(readFileSync(file));
  }
  return hash.digest("hex");
}

/**
 * Whether the built Next bundle predates the newest source edit.
 *
 * `.next/BUILD_ID` is rewritten by every build, so its mtime is the build's
 * clock. A source file newer than it means the bundle on disk — and therefore
 * an adopted `next start` serving it — does not contain that edit.
 *
 * `null` when there is no build at all: that is not staleness, it is a first
 * run, and the webServer command will build.
 */
export function nextBundleIsStale(root: string): { newest: string } | null {
  let buildAt: number;
  try {
    buildAt = statSync(path.join(root, ".next", "BUILD_ID")).mtimeMs;
  } catch {
    return null;
  }
  // Collected then reduced rather than tracked in a mutable outer variable:
  // the narrowing of a `let` assigned inside a closure is not something the
  // compiler can follow, so the "did we find one" check read as dead code.
  const candidates: { file: string; at: number }[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (/\.(tsx?|css|mjs|json)$/.test(entry.name)) {
        candidates.push({ file: full, at: statSync(full).mtimeMs });
      }
    }
  };
  walk(path.join(root, "src"));
  const newest = candidates.reduce<{ file: string; at: number } | null>(
    (best, next) => (best === null || next.at > best.at ? next : best),
    null,
  );
  if (newest === null || newest.at <= buildAt) return null;
  return { newest: path.relative(root, newest.file) };
}

/** The one remedy, named the same way everywhere. */
export const REMEDY =
  "run `pnpm test:e2e:fresh` (or `bash dev/kill-servers.sh`) so both servers restart from source";
