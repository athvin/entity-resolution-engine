/**
 * The licenses half of the frontend's supply-chain gate — the mirror of
 * `cargo deny check licenses` (rust/deny.toml `[licenses] allow`).
 *
 * An allowlist, not a denylist: a license this file has never seen fails the
 * gate and has to be looked at by a person. That is the whole point — the risk
 * is a copyleft dependency arriving unnoticed in a transitive bump.
 *
 * The first block below is rust/deny.toml's list verbatim, so the two gates
 * agree wherever they overlap. The second block is the additions this tree
 * needs; each carries the reason it is permissive enough to allow, because a
 * bare SPDX id in a list teaches the next reader nothing.
 *
 * Run locally with `pnpm licenses:check`.
 */
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

/** rust/deny.toml `[licenses] allow`, verbatim. */
const DENY_TOML_ALLOW = [
  "MIT",
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "BSL-1.0",
  "ISC",
  "Unicode-3.0",
  "Zlib",
  "CC0-1.0",
  "CDLA-Permissive-2.0",
  "0BSD",
];

/**
 * npm-tree additions. Keyed by SPDX id so the justification travels with it.
 *
 * MPL-2.0 and LGPL-3.0-or-later are the two that are not simply permissive, and
 * both are allowed on the same grounds: their copyleft is file-level (MPL) or
 * library-level (LGPL), triggered by *distributing modified copies of those
 * files* or by static linking. We consume both as unmodified, dynamically
 * loaded artifacts, so neither reaches our source. If that ever stops being
 * true — if we vendor or patch one of these — this decision has to be revisited.
 */
const NPM_ADDITIONS = {
  "MPL-2.0":
    "File-level copyleft, triggered only by distributing modified MPL files. " +
    "Present as axe-core/@axe-core/playwright (devDependency, test-only) and " +
    "lightningcss (Tailwind build-time transitive). We modify neither.",
  "LGPL-3.0-or-later":
    "@img/sharp-libvips-* — the libvips native binary behind sharp, loaded " +
    "dynamically as an unmodified prebuilt. LGPL explicitly permits this " +
    "without relicensing the application; we neither statically link nor patch it.",
  "MIT-0": "MIT without the attribution clause — strictly more permissive than MIT.",
  Unlicense: "Public-domain dedication.",
  "BlueOak-1.0.0": "OSI-approved permissive license; a plain-language MIT equivalent.",
  "Python-2.0": "Permissive and GPL-compatible (the `argparse` npm port carries it).",
  "CC-BY-4.0":
    "caniuse-lite — a browser-support data set requiring attribution only, no " +
    "obligation on the code that reads it.",
};

const ALLOWED = new Set([...DENY_TOML_ALLOW, ...Object.keys(NPM_ADDITIONS)]);

/**
 * Evaluate an SPDX expression against the allowlist. Handles the two forms that
 * actually appear in npm metadata — `(A OR B)` and `(A AND B)` — and treats
 * anything else as a single id. Deliberately not a full SPDX parser: an
 * expression this does not understand falls through to "not allowed", which
 * fails the gate and gets a human to look, which is the safe direction.
 */
function isAllowed(expression) {
  const expr = expression
    .trim()
    .replace(/^\(|\)$/g, "")
    .trim();
  if (ALLOWED.has(expr)) return true;
  if (/ OR /i.test(expr)) return expr.split(/ OR /i).some(isAllowed);
  if (/ AND /i.test(expr)) return expr.split(/ AND /i).every(isAllowed);
  return false;
}

const frontendDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const raw = execFileSync("pnpm", ["licenses", "list", "--json"], {
  cwd: frontendDir,
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
});

const byLicense = JSON.parse(raw);
const offenders = [];
let packages = 0;

for (const [license, entries] of Object.entries(byLicense)) {
  packages += entries.length;
  if (isAllowed(license)) continue;
  offenders.push({ license, entries });
}

const licenseCount = Object.keys(byLicense).length;
console.log(`licenses gate: ${packages} packages across ${licenseCount} license expressions.`);

if (!offenders.length) {
  console.log("licenses gate passed (every license is on the allowlist).");
  process.exit(0);
}

console.error("\nlicenses gate FAILED — license(s) not on the allowlist:\n");
for (const { license, entries } of offenders) {
  console.error(`  ${license}`);
  for (const entry of entries) {
    console.error(`    ${entry.name}@${entry.versions.join(", ")}`);
  }
}
console.error(
  "\nIf the license is permissive enough for us, add it to NPM_ADDITIONS in\n" +
    "scripts/check-licenses.mjs WITH the reason. If it is not, replace the dependency.",
);
process.exit(1);
