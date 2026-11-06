# Evals: sql-analysis

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. The two-repository function stub

**Query:** "My new read statement calls DuckDB's `list_sort()` and
`er-entity verify` says the function is unknown. Fix it."

Must:

- Identify that the planner is **DataFusion via ff-analysis**, which does not
  know DuckDB's function library.
- Say the fix is a **stub in feather-flow — a separate repository — plus a tag
  bump** in `er-sql-check/Cargo.toml`, and that it cannot be done in this repo
  alone.

Must not:

- Reach for `mode: partial` as the first answer.
- Suggest upgrading the workspace's `sqlparser` to gain support.

Credit for first checking whether a rewrite would do, or whether the SQL is
engine model SQL that the analyser never sees.

## 2. What `mode: partial` actually skips

**Query:** "This statement uses `SELECT *` inside a CTE and won't pass. Can I
set `mode: partial` to get around E011?"

Must:

- Say no: `partial` skips the **planner**, not the parser — every AST pass still
  runs on the full AST, E011 among them.
- Add that **E011 has no override at all**.

Must not:

- Claim the MERGE rules (E014, E015) are skipped under `partial`. They have no
  other subject — a MERGE is always partial, so skipping them would retire them
  outright.

Credit for naming the real purpose of `partial` — the planner cannot derive the
output schema, so the oracle supplies it, as with `MERGE`.

## 3. The sqlparser version question

**Query:** "We're on sqlparser 0.63 — does the SQL analyser support the syntax
added in 0.6x?"

Must:

- Correct the premise: `er-sql-check` parses only through
  `ff_analysis::sqlparser`, which is a re-export of
  `datafusion_expr::sqlparser` — **not** the workspace crate.
- Say the workspace's copy has exactly one consumer, `dedupe-core`.

Must not:

- Quote a parser version from memory. The pins are derived in
  `references/ff-analysis.md`; ff-analysis's own direct `sqlparser` dependency
  is a different, newer major that never parses a statement here, and it is the
  one `cargo tree` surfaces first.

Credit for mentioning that two arrow majors coexist deliberately — the
analyser's and the one `duckdb` pulls — and why that is contained.

## 4. The aggregate type mismatch

**Query:** "The oracle says my new statement's `total` column should be HUGEINT
but I declared bigint. The analyser was happy. Which is right?"

Must:

- Say the **oracle is the authority** — it compares against a real DuckDB.
- Identify the cause: DuckDB types `sum(BIGINT)` as `HUGEINT`; wrap it in
  `CAST(… AS BIGINT)`.
- Note aggregate results are plan-nullable, so declare nullable and
  `unwrap_or(0)` at the call site.

## 5. Config-driven SQL

**Query:** "I need a statement whose WHERE clause depends on which blocking
rules the tenant configured. How do I declare it?"

Must:

- Say it **cannot** be a declared statement, because its shape depends on
  runtime config.
- Route to a Rust generator function beside its domain crate, plus a
  **plan-vector test** planning every shape it can emit over real config
  fixtures.

Must not:

- Suggest building the SQL string inside an `er-server` handler (the
  `freeform_sql` gate forbids it).

## 6. Selection: an E0xx failure with no skill named

**Query:** "`make check` is failing with `E003: output does not match the
declaration` on a statement I just added. What do I do?"

This one tests **selection**, not knowledge. `entity-framework` and
`sql-analysis` both carry the E0xx trigger vocabulary, and the remedy table
lives here.

Must:

- Load `sql-analysis` without it being named in the prompt.
- Reach the E003 remedy: a `returns: entity` statement must project **every**
  column, in order — so adding an entity column means updating every such
  statement.

Must not:

- Offer `mode: partial` as the way to make E003 stop firing. It would work —
  `partial` skips exactly that check — which is why it is the escape-hatch
  misuse this skill exists to prevent.

## 7. A construct the parser cannot read

**Query:** "My new lake read needs `AT (VERSION => $snapshot)`, but the parser
rejects `=>`. Can I just drop the clause?"

Must:

- Say no — dropping it fails **E006**, which wants the pin unless the statement
  declares `pin: live` with a `pin_reason`.
- Explain that nothing needs to change in the SQL: the rewrite step already
  strips the clause *and records it*, so the parser never sees `=>` and E006
  still has its evidence.

Must not:

- Propose `mode: partial`, or a stub in feather-flow. The rewrite already
  covers this construct; both answers are work that is already done.
