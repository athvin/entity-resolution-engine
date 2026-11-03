#!/usr/bin/env node
/**
 * The erweb schema's migrations must be backward compatible.
 *
 * ## Why this is not a blue/green feature
 *
 * `templates/job-db-migrate.yaml` runs `node scripts/db-migrate.mjs` as a
 * `helm.sh/hook: pre-install,pre-upgrade` Job. **Pre-upgrade means the old pod
 * is still serving** — Helm applies the hook, waits for it, and only then
 * touches the Deployment. So every migration here has ALWAYS been applied
 * underneath live code, and nothing checked that it could be.
 *
 * The neighbouring Rust half makes the contrast exact. `job-tenant-migrate.yaml`
 * is `post-upgrade` and says why in its own header: *"the objection that rules
 * out a pre-upgrade hook for the control plane (the old pod still serves) does
 * not apply here."* The control plane took that objection seriously and moved
 * its migration into the pod's init container. The erweb half kept the
 * pre-upgrade hook and acquired no corresponding rule.
 *
 * The control plane also cannot execute a breaking change at all: its
 * reconciler emits `CREATE TABLE` for an absent relation and `ADD COLUMN` for
 * an absent nullable column, and refuses everything else as
 * `ERR_SCHEMA_BREAKING`. Drizzle emits whatever `drizzle-kit` generated. This
 * gate is the missing half of that asymmetry, expressed in the only place it
 * can be: the committed SQL.
 *
 * The eight migrations in the tree today are all `CREATE`, so this gate starts
 * green — it is a tripwire for the ninth, not a cleanup.
 *
 * ## What is refused
 *
 * Two severities, because they are genuinely different failures and collapsing
 * them produced a false positive on a migration that had already shipped —
 * which is how the distinction got found.
 *
 * **Hard breaks.** The old pod's SQL *fails*:
 *
 * | pattern | why |
 * |---|---|
 * | `DROP COLUMN` | its `SELECT` names that column |
 * | `DROP TABLE` / `DROP SCHEMA` | likewise |
 * | `RENAME` | a drop and an add at once, from the reader's side |
 * | `ALTER … TYPE` | the driver decodes the old type |
 * | `SET NOT NULL` | its `INSERT` omits the column |
 * | `ADD COLUMN … NOT NULL` with no `DEFAULT` | its `INSERT` does not name the column |
 *
 * An `ADD COLUMN … NOT NULL DEFAULT …` is fine and is deliberately allowed:
 * the default is what makes the old pod's existing `INSERT` statements keep
 * working. That is exactly the line `er_entity_diff`'s `Severity` draws between
 * `Safe` and `Breaking`, and this gate is the erweb mirror of it.
 *
 * **Invariant swaps.** `DROP INDEX` and `DROP CONSTRAINT` do NOT break the old
 * pod — it keeps reading and writing fine. What they remove is a guarantee its
 * code assumes. The honest rule is not "never" but "replace it": a migration
 * that drops a unique index and adds a wider one in the same transaction has a
 * window of one transaction, while one that drops it and stops has given up an
 * invariant permanently and silently.
 *
 * So a swap is a finding only when the same migration adds nothing back.
 * `0007_thick_cyclops.sql` is the worked example in the tree: it drops three
 * indexes and a primary key, and adds the dataset-scoped replacements for all
 * four. Treating that as a hard break would have forced an `er-contract` marker
 * onto something correct — and a marker demanded where it is not warranted is
 * how the escape hatch becomes reflexive.
 *
 * ## The contract escape hatch
 *
 * Removing a column is legitimate — in TWO releases. Release N stops reading
 * it; release N+1 drops it, once no running pod names it. A migration may
 * therefore opt out with a marker comment carrying the reason:
 *
 *     -- er-contract: release N+1 of the org_credentials rename; no reader
 *     -- has named `legacy_nonce` since <commit>, which shipped in <release>.
 *     ALTER TABLE "erweb"."org_credentials" DROP COLUMN "legacy_nonce";
 *
 * The marker is deliberately a nuisance to write: it has to name what stopped
 * reading the column and when. A bare `-- er-contract` with no prose after it
 * is refused, because an escape hatch nobody has to justify is just an absent
 * gate.
 */

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

/** The marker that opts a file out, and the minimum justification length. */
const CONTRACT_MARKER = "er-contract:";
const CONTRACT_MIN_REASON = 30;

/**
 * Hard breaks: the old pod's SQL fails outright. Applied to SQL with comments
 * and string literals stripped, so a pattern cannot fire on prose or a value.
 */
const HARD = [
  { re: /\bDROP\s+COLUMN\b/i, what: "DROP COLUMN" },
  { re: /\bDROP\s+TABLE\b/i, what: "DROP TABLE" },
  { re: /\bDROP\s+SCHEMA\b/i, what: "DROP SCHEMA" },
  { re: /\bRENAME\s+(COLUMN|TO|CONSTRAINT)\b/i, what: "RENAME" },
  { re: /\bALTER\s+COLUMN\s+"?[\w]+"?\s+(SET\s+DATA\s+)?TYPE\b/i, what: "ALTER COLUMN … TYPE" },
  { re: /\bSET\s+NOT\s+NULL\b/i, what: "SET NOT NULL" },
];

/**
 * Invariant swaps: safe when the same migration adds a replacement, a silent
 * loss of a guarantee when it does not. See the header.
 */
const SWAP = [
  { re: /\bDROP\s+CONSTRAINT\b/i, what: "DROP CONSTRAINT", adds: /\bADD\s+CONSTRAINT\b/i },
  { re: /\bDROP\s+INDEX\b/i, what: "DROP INDEX", adds: /\bCREATE\s+(UNIQUE\s+)?INDEX\b/i },
];

/**
 * Strip `--` line comments and single-quoted literals.
 *
 * Both matter. A comment explaining a past `DROP COLUMN` would otherwise be
 * read as one — the mistake `bff-envelope-contract.test.ts` records having made
 * in its first draft, where a comment quoting a historical bug was matched as
 * the bug. And a default value like `'dropped'` must not look like a statement.
 *
 * @param {string} sql
 * @returns {string}
 */
export function stripNoise(sql) {
  let out = "";
  let i = 0;
  /** @type {"code" | "line" | "quote" | "dollar"} */
  let mode = "code";
  while (i < sql.length) {
    const two = sql.slice(i, i + 2);
    if (mode === "code") {
      if (two === "--") {
        mode = "line";
        i += 2;
        continue;
      }
      if (sql.startsWith("$$", i)) {
        mode = "dollar";
        i += 2;
        continue;
      }
      if (sql.charAt(i) === "'") {
        mode = "quote";
        i += 1;
        out += " ";
        continue;
      }
      out += sql.charAt(i);
      i += 1;
      continue;
    }
    if (mode === "line") {
      if (sql.charAt(i) === "\n") {
        mode = "code";
        out += "\n";
      }
      i += 1;
      continue;
    }
    if (mode === "dollar") {
      if (sql.startsWith("$$", i)) {
        mode = "code";
        i += 2;
      } else {
        i += 1;
      }
      continue;
    }
    // inside a single-quoted literal: '' is an escaped quote, not the end
    if (two === "''") {
      i += 2;
      continue;
    }
    if (sql.charAt(i) === "'") {
      mode = "code";
    }
    i += 1;
  }
  return out;
}

/**
 * An `ADD COLUMN … NOT NULL` with no `DEFAULT` in the same clause.
 *
 * Scoped to the clause rather than the statement: `ADD COLUMN a NOT NULL,
 * ADD COLUMN b DEFAULT 1` must not be excused by b's default. Drizzle emits
 * one `ADD COLUMN` per `ALTER TABLE` line in practice, so this is belt and
 * braces — but the belt is cheap and the failure is silent.
 *
 * @param {string} sql already stripped of comments and literals
 * @returns {string[]} the offending clauses
 */
export function nullabilityOffenders(sql) {
  const out = [];
  // Each ADD COLUMN clause runs to the next ADD/ALTER/DROP/ end of statement.
  const clauses = sql.match(/\bADD\s+COLUMN\b[^;,]*/gi) ?? [];
  for (const clause of clauses) {
    if (!/\bNOT\s+NULL\b/i.test(clause)) continue;
    if (/\bDEFAULT\b/i.test(clause)) continue;
    if (/\bGENERATED\b/i.test(clause)) continue; // a generated column computes its own value
    out.push(clause.trim().replace(/\s+/g, " "));
  }
  return out;
}

/**
 * Does this file carry a justified contract marker?
 *
 * @param {string} raw the ORIGINAL sql, comments intact
 * @returns {{ exempt: boolean, reason: string }}
 */
export function contractExemption(raw) {
  const at = raw.indexOf(CONTRACT_MARKER);
  if (at === -1) return { exempt: false, reason: "" };

  // The rest of the marker's own line, then CONTINUED `--` lines only.
  //
  // Stopping at the first non-comment line is load-bearing, and getting it
  // wrong is how the first version of this passed its own tests: reading every
  // following line meant the SQL statement itself counted toward the reason, so
  // `-- er-contract: needed` plus any `ALTER TABLE …` cleared the length bar.
  // The justification has to be prose a person wrote, not the statement it is
  // excusing.
  const [first, ...rest] = raw.slice(at + CONTRACT_MARKER.length).split("\n");
  const parts = [(first ?? "").trim()];
  for (const line of rest) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("--")) break;
    parts.push(trimmed.replace(/^--\s?/, "").trim());
  }
  const reason = parts.filter(Boolean).join(" ").trim();
  return { exempt: reason.length >= CONTRACT_MIN_REASON, reason };
}

/**
 * Every finding across the migration directory, plus how many files were read.
 *
 * `examined` is returned rather than logged because it is what `main` turns
 * into the rename-detection exit: a gate that reports success having read zero
 * files is worse than no gate.
 *
 * @param {string} dir
 * @returns {{
 *   findings: {file: string, what: string, detail: string}[],
 *   examined: number,
 * }}
 */
export function findBreakingMigrations(dir) {
  const findings = [];
  const files = readdirSync(dir)
    .filter((name) => name.endsWith(".sql"))
    .sort();
  for (const name of files) {
    const raw = readFileSync(path.join(dir, name), "utf8");
    const { exempt, reason } = contractExemption(raw);
    const sql = stripNoise(raw);

    const hits = HARD.filter((rule) => rule.re.test(sql)).map((rule) => rule.what);
    for (const clause of nullabilityOffenders(sql)) {
      hits.push(`ADD COLUMN … NOT NULL with no DEFAULT (${clause})`);
    }
    // A swap counts only when nothing is added back in the same migration.
    for (const rule of SWAP) {
      if (!rule.re.test(sql)) continue;
      if (rule.adds.test(sql)) {
        console.log(`  ${name}: ${rule.what} with a replacement in the same migration — fine`);
        continue;
      }
      hits.push(`${rule.what} with no replacement in the same migration`);
    }
    if (hits.length === 0) continue;

    if (exempt) {
      console.log(`  ${name}: ${hits.join(", ")} — exempt: ${reason.slice(0, 120)}`);
      continue;
    }
    for (const what of hits) {
      findings.push({
        file: name,
        what,
        detail: raw.indexOf(CONTRACT_MARKER) === -1 ? "no contract marker" : "marker has no reason",
      });
    }
  }
  return { findings, examined: files.length };
}

function main() {
  const dir = path.join(path.resolve(import.meta.dirname, ".."), "drizzle");
  const { findings, examined } = findBreakingMigrations(dir);

  // Rename detection, the property every gate here needs: a glob that stops
  // matching reports success having checked nothing, which is strictly worse
  // than no gate at all.
  if (examined === 0) {
    console.error(
      "migration gate: examined 0 .sql files in frontend/drizzle — the directory\n" +
        "must have moved. Fix the path in scripts/check-migrations.mjs; do not let\n" +
        "this pass as green.",
    );
    process.exit(2);
  }

  if (findings.length > 0) {
    console.error(
      `\nmigration gate FAILED — ${String(findings.length)} backward-incompatible ` +
        `statement(s) in frontend/drizzle:\n`,
    );
    for (const { file, what, detail } of findings) {
      console.error(`  ${file}: ${what} (${detail})`);
    }
    console.error(
      "\nThese migrations run as a helm PRE-UPGRADE hook, so they apply while the\n" +
        "OLD pod is still serving. A dropped or retyped column breaks it mid-request.\n" +
        "\n" +
        "Removing a column takes TWO releases: N stops reading it, N+1 drops it.\n" +
        "If this IS release N+1, opt out with a marker naming what stopped reading\n" +
        "it and when:\n" +
        "\n" +
        "  -- er-contract: release N+1 of <change>; no reader has named <column>\n" +
        "  -- since <commit>, which shipped in <release>.\n",
    );
    process.exit(1);
  }

  console.log(`migration gate passed: ${String(examined)} migration(s), all backward compatible.`);
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  main();
}
