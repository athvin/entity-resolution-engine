/**
 * The advisories half of the frontend's supply-chain gate — the mirror of
 * `cargo deny check advisories` on the Rust side (rust/deny.toml).
 *
 * Fails on any HIGH or CRITICAL advisory in the resolved dependency tree.
 * Moderate and below are reported by the `pnpm audit` step in the workflow but
 * do not fail the build: this tree carries a long tail of dev-only transitive
 * DoS advisories, and a gate that cries wolf gets switched off.
 *
 * Exceptions live in audit-allow.json and follow deny.toml's house convention —
 * a prose justification per entry naming why the advisory cannot reach us. An
 * entry that no longer matches anything is reported as stale, so the allowlist
 * does not quietly accumulate.
 *
 * Run locally with `pnpm audit:check` (needs network, like `cargo deny`).
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const FAIL_ON = new Set(["high", "critical"]);

const frontendDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const allowPath = path.join(frontendDir, "audit-allow.json");

/** `pnpm audit` exits non-zero whenever it finds anything, so the status is not
 * a signal — but empty or unparseable output means the audit itself failed, and
 * that must not pass as "no vulnerabilities". */
function runAudit() {
  let stdout;
  try {
    stdout = execFileSync("pnpm", ["audit", "--json"], {
      cwd: frontendDir,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    });
  } catch (error) {
    stdout = error.stdout ?? "";
    if (!stdout.trim()) {
      console.error("audit gate: `pnpm audit --json` produced no output.");
      console.error(error.stderr || error.message);
      process.exit(2);
    }
  }
  try {
    return JSON.parse(stdout);
  } catch {
    console.error("audit gate: could not parse `pnpm audit --json` output.");
    process.exit(2);
  }
}

const { allow = [] } = JSON.parse(readFileSync(allowPath, "utf8"));
const allowById = new Map(allow.map((entry) => [String(entry.id), entry]));
for (const [id, entry] of allowById) {
  if (!entry.reason?.trim()) {
    console.error(`audit gate: allowlist entry ${id} has no \`reason\`.`);
    process.exit(2);
  }
}

const report = runAudit();
const advisories = Object.values(report.advisories ?? {});
const counts = report.metadata?.vulnerabilities ?? {};

const gated = advisories.filter((a) => FAIL_ON.has(a.severity));
const matched = new Set();
const failures = [];

for (const advisory of gated) {
  const id = String(advisory.id);
  const exception = allowById.get(id);
  const via = advisory.findings?.[0]?.paths?.[0] ?? "(path unknown)";
  if (exception) {
    matched.add(id);
    console.log(`allowed  ${advisory.severity.padEnd(8)} ${id} ${advisory.module_name}`);
    console.log(`         ${exception.reason}`);
    continue;
  }
  failures.push({ id, advisory, via });
}

const stale = [...allowById.keys()].filter((id) => !matched.has(id));
if (stale.length) {
  console.log(`\nstale allowlist entries (no longer reported — delete them): ${stale.join(", ")}`);
}

console.log(
  `\n${advisories.length} advisories: ` +
    ["critical", "high", "moderate", "low", "info"]
      .map((level) => `${counts[level] ?? 0} ${level}`)
      .join(", "),
);

if (!failures.length) {
  console.log(`audit gate passed (no unapproved ${[...FAIL_ON].join("/")} advisories).`);
  process.exit(0);
}

console.error(`\naudit gate FAILED — ${failures.length} unapproved advisory(ies):\n`);
for (const { id, advisory, via } of failures) {
  console.error(`  ${advisory.severity.toUpperCase()} ${id} — ${advisory.module_name}`);
  console.error(`    ${advisory.title}`);
  console.error(
    `    vulnerable ${advisory.vulnerable_versions} / patched ${advisory.patched_versions}`,
  );
  console.error(`    via ${via}`);
  console.error(`    ${advisory.url ?? ""}`);
}
console.error(
  "\nFix by upgrading (or a `pnpm.overrides` pin when the fix is in a transitive dep).\n" +
    "If it genuinely cannot reach us, add it to audit-allow.json with a reason.",
);
process.exit(1);
