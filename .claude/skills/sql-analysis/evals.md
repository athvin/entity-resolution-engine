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

- Say no: `partial` skips the **planner**, not the parser, and
  E004/E005/E010/E011/E012 still run on the full AST.
- Add that **E011 has no override at all**.

Credit for naming the real purpose of `partial` — the planner cannot derive the
output schema, so the oracle supplies it, as with `MERGE`.

## 3. The sqlparser version question

**Query:** "We're on sqlparser 0.63 — does the SQL analyser support the syntax
added in 0.6x?"

Must:

- Correct the premise: `er-sql-check` parses only through
  `ff_analysis::sqlparser`, which is **DataFusion 52's bundled 0.59**.
- Say the workspace's 0.63 has exactly one consumer, `dedupe-core`.

Credit for mentioning that arrow 57 (ff-analysis) and arrow 58 (duckdb) coexist
deliberately, and why that is contained.

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
