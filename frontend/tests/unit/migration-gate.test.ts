import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  contractExemption,
  findBreakingMigrations,
  nullabilityOffenders,
  stripNoise,
} from "../../scripts/check-migrations.mjs";

/**
 * The gate in `scripts/check-migrations.mjs` asserts that every committed
 * drizzle migration can be applied while the PREVIOUS release is still
 * serving — which is not a hypothetical property but the literal deployment
 * order: `templates/job-db-migrate.yaml` is a `helm.sh/hook: pre-upgrade`, so
 * Helm applies it and waits before it touches the Deployment.
 *
 * Every case here plants a violation and requires the gate to report it. The
 * reason that discipline is non-negotiable in this repo is written out in
 * `bff-authz-gate.test.ts`: the first version of that invariant was a semgrep
 * rule whose pattern failed to parse, and semgrep reported it as run with zero
 * findings — a silent false green over the property it existed to protect.
 */

let dir: string | null = null;

function migrations(files: Record<string, string>): string {
  dir = mkdtempSync(path.join(tmpdir(), "migration-gate-"));
  for (const [name, sql] of Object.entries(files)) {
    writeFileSync(path.join(dir, name), sql);
  }
  return dir;
}

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = null;
});

describe("hard breaks are refused", () => {
  it.each([
    ["DROP COLUMN", `ALTER TABLE "erweb"."users" DROP COLUMN "email";`],
    ["DROP TABLE", `DROP TABLE "erweb"."sessions";`],
    ["DROP SCHEMA", `DROP SCHEMA "erweb" CASCADE;`],
    ["RENAME", `ALTER TABLE "erweb"."users" RENAME COLUMN "email" TO "address";`],
    ["ALTER COLUMN … TYPE", `ALTER TABLE "erweb"."users" ALTER COLUMN "id" SET DATA TYPE bigint;`],
    ["SET NOT NULL", `ALTER TABLE "erweb"."users" ALTER COLUMN "org" SET NOT NULL;`],
  ])("%s", (_label, sql) => {
    const { findings, examined } = findBreakingMigrations(migrations({ "0001_x.sql": sql }));
    expect(examined).toBe(1);
    expect(findings).toHaveLength(1);
  });

  it("refuses ADD COLUMN NOT NULL with no DEFAULT", () => {
    // The old pod's INSERT does not name the new column, so it fails.
    const { findings } = findBreakingMigrations(
      migrations({ "0001_x.sql": `ALTER TABLE "erweb"."users" ADD COLUMN "org" text NOT NULL;` }),
    );
    expect(findings).toHaveLength(1);
    expect(findings[0]?.what).toContain("NOT NULL with no DEFAULT");
  });

  it("ALLOWS ADD COLUMN NOT NULL with a DEFAULT", () => {
    // The default is what keeps the old pod's existing INSERTs working — the
    // same line er_entity_diff's Severity draws between Safe and Breaking.
    const { findings } = findBreakingMigrations(
      migrations({
        "0001_x.sql": `ALTER TABLE "erweb"."users" ADD COLUMN "org" text DEFAULT 'default' NOT NULL;`,
      }),
    );
    expect(findings).toHaveLength(0);
  });

  it("allows a plain nullable ADD COLUMN", () => {
    const { findings } = findBreakingMigrations(
      migrations({ "0001_x.sql": `ALTER TABLE "erweb"."users" ADD COLUMN "note" text;` }),
    );
    expect(findings).toHaveLength(0);
  });
});

describe("invariant swaps turn on whether something is added back", () => {
  it("allows a DROP INDEX that is replaced in the same migration", () => {
    // 0007_thick_cyclops.sql in the real tree: drops three indexes and a
    // primary key, adds the dataset-scoped replacements for all four. The
    // window is one transaction.
    const { findings } = findBreakingMigrations(
      migrations({
        "0007_x.sql": `DROP INDEX "erweb"."segments_org_idx";
CREATE INDEX "segments_org_ds_idx" ON "erweb"."segments" USING btree ("org","dataset");`,
      }),
    );
    expect(findings).toHaveLength(0);
  });

  it("refuses a DROP INDEX that replaces nothing", () => {
    // An invariant given up permanently and silently.
    const { findings } = findBreakingMigrations(
      migrations({ "0001_x.sql": `DROP INDEX "erweb"."segments_org_idx";` }),
    );
    expect(findings).toHaveLength(1);
    expect(findings[0]?.what).toContain("no replacement");
  });

  it("allows a DROP CONSTRAINT that is replaced, refuses one that is not", () => {
    const replaced = findBreakingMigrations(
      migrations({
        "0001_x.sql": `ALTER TABLE "erweb"."r" DROP CONSTRAINT "r_pk";
ALTER TABLE "erweb"."r" ADD CONSTRAINT "r_pk2" PRIMARY KEY("org","dataset");`,
      }),
    );
    expect(replaced.findings).toHaveLength(0);

    const bare = findBreakingMigrations(
      migrations({ "0002_x.sql": `ALTER TABLE "erweb"."r" DROP CONSTRAINT "r_pk";` }),
    );
    expect(bare.findings).toHaveLength(1);
  });
});

describe("the contract escape hatch", () => {
  const DROP = `ALTER TABLE "erweb"."users" DROP COLUMN "legacy";`;

  it("exempts a drop whose marker carries a real justification", () => {
    const { findings } = findBreakingMigrations(
      migrations({
        "0009_x.sql": `-- er-contract: release N+1 of the legacy-column removal; no reader
-- has named "legacy" since commit abc1234, which shipped two releases ago.
${DROP}`,
      }),
    );
    expect(findings).toHaveLength(0);
  });

  it("refuses a bare marker with no reason", () => {
    // An escape hatch nobody has to justify is just an absent gate.
    const { findings } = findBreakingMigrations(
      migrations({ "0009_x.sql": `-- er-contract:\n${DROP}` }),
    );
    expect(findings).toHaveLength(1);
    expect(findings[0]?.detail).toBe("marker has no reason");
  });

  it("refuses a marker whose reason is too short to say anything", () => {
    const { findings } = findBreakingMigrations(
      migrations({ "0009_x.sql": `-- er-contract: needed\n${DROP}` }),
    );
    expect(findings).toHaveLength(1);
  });

  it("reports the absence of a marker distinctly from an empty one", () => {
    const { findings } = findBreakingMigrations(migrations({ "0009_x.sql": DROP }));
    expect(findings[0]?.detail).toBe("no contract marker");
  });
});

describe("comments and literals cannot trigger the gate", () => {
  it("ignores a breaking statement quoted in a comment", () => {
    // This file's own header quotes `DROP COLUMN` while explaining it, which is
    // the exact mistake bff-envelope-contract.test.ts records making in its
    // first draft: a comment describing a bug read as the bug.
    const { findings } = findBreakingMigrations(
      migrations({
        "0001_x.sql": `-- supersedes the old DROP COLUMN approach
ALTER TABLE "erweb"."users" ADD COLUMN "note" text;`,
      }),
    );
    expect(findings).toHaveLength(0);
  });

  it("ignores a breaking phrase inside a string literal", () => {
    const { findings } = findBreakingMigrations(
      migrations({
        "0001_x.sql": `ALTER TABLE "erweb"."u" ADD COLUMN "s" text DEFAULT 'DROP TABLE x' NOT NULL;`,
      }),
    );
    expect(findings).toHaveLength(0);
  });

  it("strips line comments and literals but keeps the code", () => {
    const stripped = stripNoise(
      `SELECT 1; -- DROP TABLE t\nINSERT INTO u VALUES ('DROP TABLE v');`,
    );
    expect(stripped).not.toContain("DROP TABLE t");
    expect(stripped).not.toContain("DROP TABLE v");
    expect(stripped).toContain("SELECT 1");
    expect(stripped).toContain("INSERT INTO u");
  });

  it("does not end a literal on an escaped quote", () => {
    // `'it''s'` is one literal. Ending it early would expose its tail as code.
    expect(stripNoise(`VALUES ('it''s DROP TABLE t');`)).not.toContain("DROP TABLE");
  });
});

describe("the gate cannot pass by examining nothing", () => {
  it("reports zero examined files for an empty directory", () => {
    // main() turns this into exit 2 rather than success — the rename-detection
    // property. Asserted here because the directory moving is the realistic
    // way a gate stops covering anything.
    const { examined, findings } = findBreakingMigrations(migrations({}));
    expect(examined).toBe(0);
    expect(findings).toHaveLength(0);
  });

  it("ignores non-sql files so a stray artifact cannot inflate the count", () => {
    const { examined } = findBreakingMigrations(
      migrations({ "0001_x.sql": "SELECT 1;", "README.md": "DROP TABLE everything;" }),
    );
    expect(examined).toBe(1);
  });
});

describe("the helpers behave on their own", () => {
  it("scopes nullability to the clause, not the statement", () => {
    // `ADD COLUMN a NOT NULL, ADD COLUMN b DEFAULT 1` must not be excused by
    // b's default.
    const offenders = nullabilityOffenders(
      `ALTER TABLE t ADD COLUMN a text NOT NULL, ADD COLUMN b text DEFAULT 'x' NOT NULL;`,
    );
    expect(offenders).toHaveLength(1);
    expect(offenders[0]).toContain('"a"'.replace(/"/g, "") || "a");
  });

  it("allows a generated column to be NOT NULL without a default", () => {
    expect(
      nullabilityOffenders(
        `ALTER TABLE t ADD COLUMN n int GENERATED ALWAYS AS (1) STORED NOT NULL`,
      ),
    ).toHaveLength(0);
  });

  it("reads a multi-line marker reason", () => {
    const { exempt, reason } = contractExemption(
      `-- er-contract: the first line\n-- and the second line, together long enough\nDROP TABLE t;`,
    );
    expect(exempt).toBe(true);
    expect(reason).toContain("the second line");
  });
});
