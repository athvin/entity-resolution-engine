# ff-analysis (feather-flow): the planner behind the analyser

## Contents

- What it is and where it lives
- The version matrix, and which pins are not free
- What the stubs cover today
- Adding a function the planner does not know
- Why the arrow split is safe
- Symptoms that point here

## What it is and where it lives

`er-sql-check` does not implement a SQL planner. It plans through
**feather-flow**, a separate first-party repository, consumed as:

```toml
ff-analysis = { git = "https://github.com/athvin/feather-flow", tag = "er-entity-v0.1.4" }
```

It is pinned **by tag**, not by branch or revision range — so the planner this
repo analyses with is a fixed, named artifact, and changing it is an explicit
commit here.

ff-analysis provides `plan_statement` (taking a **pre-parsed AST**, which is
what lets the rewrite step run first), a `FeatherFlowProvider` catalog, and the
DuckDB function stubs DataFusion otherwise lacks.

## The version matrix, and which pins are not free

| Crate | Version | Source |
|---|---|---|
| DataFusion | 52 | brought by ff-analysis |
| arrow | 57 | brought by ff-analysis |
| sqlparser | 0.59 | DataFusion's bundled copy, reached as `ff_analysis::sqlparser` |
| `datafusion-common`, `datafusion-expr` | 52 | pinned here to match |

Two statements that are commonly got wrong:

- **`er-sql-check` has no dependency on the standalone `sqlparser` crate.** The
  workspace's `sqlparser = "0.63"` has exactly one consumer, `dedupe-core`. So
  "the analyser uses sqlparser 0.63" is false.
- **The arrow major here (57) differs from the rest of the workspace (58).**
  The workspace tracks the major the `duckdb` crate pulls.

`datafusion-common` and `datafusion-expr` are pinned to the same major
ff-analysis resolves so that the `LogicalPlan` this crate inspects and the plan
ff-analysis produces are the *same types*. Bumping them independently does not
produce a version conflict — it produces a type mismatch.

Dependabot is configured to leave `arrow` and `datafusion` alone for exactly
this reason: a PR bumping either alone can never go green, and the unblocker is
upstream.

## What the stubs cover today

DataFusion knows standard SQL, not DuckDB's library. ff-analysis carries stubs
so the analyser can type-check calls the engine really makes. By tag:

| Added | Covers |
|---|---|
| baseline | store-qualified `get_table_source`, `plan_statement` over a pre-parsed AST; DuckDB json/list stubs — `to_json`, `map_from_entries`, `json_keys`, `array_length`, `now`, `list` |
| v0.1.2 | the golden-lineage constructs: a custom `ExprPlanner` for DuckDB `MAP {…}` literals and subscript access, window-function stubs (`row_number`, `rank`, `lag`, …), `concat` / `concat_ws` / `element_at` |
| v0.1.3 | the standardization-stage surface: json payload accessors, `LIST(VARCHAR)`-typed list shaping, unicode normalizers, variadic regexp forms, array-literal planning |

A stub exists to make a call **plannable and typed**. It does not reimplement
the function's behaviour — correctness of the SQL's results is the oracle's and
the goldens' job, not the planner's.

Note `source_metadata` lambdas are rewritten out by `er-sql-check` before
planning, rather than being stubbed.

## Adding a function the planner does not know

This is a **two-repository change**. There is no way to do it from this repo
alone, and that is the single most useful thing to know before starting.

1. Confirm the function is genuinely needed. If the statement is engine model
   SQL inside `dedupe-core`, it is golden-governed and the analyser never sees
   it — no stub required.
2. Check whether a rewrite is the better answer. If the construct carries no
   type information the analyser needs (named argument prefixes, locking
   clauses), stripping it in `rewrite.rs` is cheaper than a stub.
3. Otherwise add the stub in **feather-flow**: a scalar UDF, aggregate, window
   function, or an `ExprPlanner` for syntax DataFusion cannot express.
4. Tag feather-flow, then bump the `tag =` in
   `rust/crates/er-sql-check/Cargo.toml` in this repo.
5. Run `make check` — `entities-check` plans every statement, so a missing or
   wrong stub shows up there.

Plan for the round trip. A change that needs a new stub is not a one-commit
change here.

## Why the arrow split is safe

Two arrow majors in one build sounds alarming. It is contained because the
types never meet:

- The analyser works in **`ff_analysis::SqlType`**, its own type vocabulary.
- The oracle compares **type-name strings** against what DuckDB reports.

Neither path hands an `arrow` value from the ff-analysis side to the duckdb
side. So the split is a build-graph fact, not a data-flow one, and it is
deliberately confined to this crate.

If you find yourself wanting to pass an `arrow::RecordBatch` across that line,
stop — that is the change that would make the split unsafe.

## Symptoms that point here

| Symptom | Likely cause |
|---|---|
| "Invalid function" / unknown function while planning a statement | a DuckDB function with no stub — needs the feather-flow round trip |
| the planner rejects named arguments | the `name :=` rewrite did not cover your call shape |
| a type mismatch between the analyser and the oracle | the stub's declared return type does not match DuckDB's real one |
| a `LogicalPlan` type error after a dependency bump | `datafusion-common`/`-expr` no longer match the major ff-analysis resolves |
| arrow version conflict in `cargo tree` | expected; confirm nothing is passing arrow types across the boundary |

When a statement will not plan, decide between three outcomes before writing
code: a rewrite here, a stub upstream, or `mode: partial` with the oracle
supplying the schema. Reaching for `partial` first is the tempting wrong answer
— it is the right one only when the planner genuinely cannot derive the output
schema, as with `MERGE`.
