/**
 * The mock-fabrication gate: the mock may not invent fields on an entity row.
 *
 * `wire-contract.test.ts` checks the other direction — that a **UI type** does
 * not declare fields its entity lacks. Neither it nor
 * `er-server/tests/mock_contract.rs` looks at what the mock actually *emits*,
 * and that is where the defect is born: the mock encodes the frontend's belief
 * about the API, so a field invented here reads as real in all three
 * mock-backed tiers. `JobRow.progress` is the worked example — declared
 * required in the UI type, fabricated by the mock at 11 sites, produced by no
 * backend, and it threw on the run detail page, the runs list and the
 * dashboard's active-job line against every real deployment while CI was green.
 *
 * This gate found two more of the same kind sitting in the same object:
 * `exit_code` (11 sites) and `outcome` (11 sites) on the mock's job fixtures,
 * neither a column on any entity and neither produced anywhere in `rust/`.
 * They were Python-era leftovers — the subprocess exit status and a job
 * outcome string the Rust dispatcher never had. Nothing read them yet, so
 * nothing was broken; they were a loaded trap, because the next UI to show
 * `job.exit_code` would have passed every tier and shipped a field that is
 * always `undefined`.
 *
 * The authority is `entities/<name>/entity.yaml` — the source of truth the
 * Rust structs, the DDL and the TypeScript bindings are all generated from, so
 * it cannot disagree with what the server serializes. All 36 are read, not
 * just the 18 with TypeScript bindings, because the lake-only entities
 * (`assertion` among them) have no binding and their rows would otherwise be
 * misattributed to whichever entity happened to share the most fields.
 *
 * ## What this does NOT cover, and why
 *
 * An object is matched to an entity by **shared field count**, not by the
 * route that serves it, because the mock dispatches on regex-matched paths and
 * builds many rows inline rather than from `initialState()`. So a field that
 * is legitimate on entity A and fabricated on entity B is only caught when B
 * is the better match. Route-accurate attribution needs the OpenAPI document
 * to describe paths, and `src/lib/api/openapi.json` documents **zero** of them
 * (`er-server/src/openapi.rs` registers `components.schemas` only, by a
 * recorded decision). That is the same limit `wire-contract.test.ts` records
 * for envelope and ordering mismatches.
 *
 * Server-composed fields are legitimate and listed in `EXPLAINED` with where
 * each comes from. The point of the gate is that such a field is *accounted
 * for* rather than assumed — an entry here is a claim someone checked.
 */
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";

import ts from "typescript";
import { parse } from "yaml";
import { describe, expect, it } from "vitest";

const REPO = path.resolve(__dirname, "../../..");
const ENTITIES = path.join(REPO, "entities");
const MOCK = path.join(REPO, "frontend/tests/mock-erserver/server.mjs");

/** An object is treated as an entity row once it shares this many fields. */
const ROW_THRESHOLD = 4;

/**
 * Fields the mock emits that no entity declares, each with the reason it is
 * real. `key` is `<best-matching entity>.<field>`.
 */
const EXPLAINED: Record<string, string> = {
  "org.resources":
    "the DuckDB parallelism view composed by update_resources (er-server/src/lib.rs), not a column",
  "metric_snapshot.duplicate_rate":
    "derived on read in er-server/src/read.rs — O7a's rule is that rates are computed, never stored",
  "metric_snapshot.records_per_entity": "derived on read in er-server/src/read.rs, as above",
  "metric_snapshot_source.duplicate_rate": "derived on read in er-server/src/read.rs, as above",
  "assertion.assertion_id":
    "a wire rename: the entity's `id` column is serialized as `assertion_id` (er-server/src/lib.rs:5974)",
  "run.auto_merged_pairs":
    "derived on read from the auto_merge threshold (er-server/src/read.rs:1020), asserted by pg_golden_read.rs",
};

/** entity name -> declared column names, from the generated-from source of truth. */
function entityColumns(): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  for (const dir of readdirSync(ENTITIES)) {
    const yml = path.join(ENTITIES, dir, "entity.yaml");
    let raw: string;
    try {
      raw = readFileSync(yml, "utf8");
    } catch {
      continue;
    }
    const doc = parse(raw) as { name?: string; columns?: { name?: string }[] };
    const cols = (doc.columns ?? []).map((c) => c.name).filter((n): n is string => !!n);
    if (doc.name && cols.length) out.set(doc.name, new Set(cols));
  }
  return out;
}

interface MockObject {
  keys: string[];
  line: number;
}

/** Every object literal in the mock, with its source line. */
function mockObjects(): MockObject[] {
  const src = readFileSync(MOCK, "utf8");
  const sf = ts.createSourceFile(MOCK, src, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
  const found: MockObject[] = [];
  const walk = (node: ts.Node): void => {
    if (ts.isObjectLiteralExpression(node)) {
      const keys: string[] = [];
      for (const prop of node.properties) {
        if (
          (ts.isPropertyAssignment(prop) || ts.isShorthandPropertyAssignment(prop)) &&
          (ts.isIdentifier(prop.name) || ts.isStringLiteral(prop.name))
        ) {
          keys.push(prop.name.text);
        }
      }
      if (keys.length) {
        found.push({
          keys,
          line: sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1,
        });
      }
    }
    ts.forEachChild(node, walk);
  };
  walk(sf);
  return found;
}

interface Finding {
  entity: string;
  field: string;
  line: number;
}

/** Foreign keys on objects that model an entity, one entry per site. */
function fabrications(): Finding[] {
  const entities = entityColumns();
  const out: Finding[] = [];
  for (const obj of mockObjects()) {
    let best: { entity: string; shared: number; foreign: string[] } | null = null;
    for (const [entity, cols] of entities) {
      const shared = obj.keys.filter((k) => cols.has(k)).length;
      const foreign = obj.keys.filter((k) => !cols.has(k));
      if (foreign.length === 0 && shared >= ROW_THRESHOLD) {
        best = { entity, shared, foreign: [] };
        break;
      }
      if (!best || shared > best.shared) best = { entity, shared, foreign };
    }
    if (!best || best.shared < ROW_THRESHOLD) continue;
    for (const field of best.foreign) {
      out.push({ entity: best.entity, field, line: obj.line });
    }
  }
  return out;
}

describe("the mock may not invent fields on an entity row", () => {
  it("reads the entity definitions as the authority", () => {
    const entities = entityColumns();
    // All 36 entity directories carry a definition; a parser that silently
    // matched none would make every assertion below vacuous.
    expect(entities.size).toBeGreaterThanOrEqual(30);
    expect(entities.get("job")).toContain("max_attempts");
    expect(entities.get("job")).not.toContain("exit_code");
  });

  it("parses the mock into object literals", () => {
    // A parse failure yields zero objects, which would also pass vacuously.
    expect(mockObjects().length).toBeGreaterThan(100);
  });

  it("emits no field the entities do not declare", () => {
    const unexplained = fabrications().filter((f) => !(`${f.entity}.${f.field}` in EXPLAINED));
    const report = [...new Set(unexplained.map((f) => `${f.entity}.${f.field}`))]
      .sort()
      .map((k) => {
        const lines = unexplained.filter((f) => `${f.entity}.${f.field}` === k).map((f) => f.line);
        return `  ${k} — mock lines ${lines.join(", ")}`;
      })
      .join("\n");
    expect(
      unexplained,
      `the mock emits ${String(unexplained.length)} field(s) that no entity declares:\n${report}\n\n` +
        "Either the field is real — add it to the entity and regenerate — or the mock is " +
        "inventing it, which makes every mock-backed tier green while the UI reads a field " +
        "no backend sends. If it is composed by the server or the BFF, add it to EXPLAINED " +
        "with where it comes from.",
    ).toEqual([]);
  });

  it("every EXPLAINED entry still describes a real emission", () => {
    // A stale allowlist is how a real finding hides: an entry kept after the
    // field stopped being emitted silently also permits it to return.
    const live = new Set(fabrications().map((f) => `${f.entity}.${f.field}`));
    const stale = Object.keys(EXPLAINED).filter((k) => !live.has(k));
    expect(
      stale,
      `EXPLAINED carries ${String(stale.length)} entry(s) the mock no longer emits: ${stale.join(", ")}. ` +
        "Remove them, so the list stays a record of what was actually checked.",
    ).toEqual([]);
  });
});
