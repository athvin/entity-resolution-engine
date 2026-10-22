---
name: sql-analysis
description: Explains the static SQL analyser (`er-sql-check`) that plans every declared statement in this repo — the E0xx diagnostics and their remedies, the rewrite step that strips what the planner cannot parse, what `mode: partial` does and does not skip, the two-store catalog, the execution oracle that checks declared types against a real database, and the ff-analysis (feather-flow) dependency that supplies the planner and its DuckDB function stubs. Use when an E0xx diagnostic fails, when a statement will not plan, when adding SQL that calls a DuckDB function the analyser does not know, when the oracle disagrees with a declared type, or when deciding between a declared statement and a plan-vector-tested generator. For writing the entity or statement YAML itself see entity-framework.
---

# The SQL analyser

Every persistent SQL statement in this repo is planned before it ships.
`er-sql-check` loads the statement YAML, rewrites what the planner cannot read,
plans each statement against a catalog built from `entities/` plus the engine's
relation registry, runs the E0xx passes, and emits the typed Rust query
functions and the oracle.

That is why a statement's declared output and the code's types cannot drift
from what the database actually returns.

## The diagnostics

The full list — implemented codes with their invariants, and the
documented-but-unimplemented ones — is generated from source so it cannot go
stale: [references/diagnostics.md](references/diagnostics.md).

Three facts about the family that are easy to get wrong:

- `E007`, `E008` and `E013` are **documented but never implemented**. Do not
  rely on them firing. (That drift is the stated reason `docs/entity-lint-rules.md`
  is now generated and byte-compared.)
- `E011` (no `SELECT *`) and `E012` (kind matches folder) have **no override**.
  There is no mode or flag that relaxes them.
- The `E0xx` family is statement-level and lives in `er-sql-check`. The `V0xx`
  family is entity-level and lives in `er-entity-lint`. They are separate
  registries with separate docs; `er-entity lint --explain V4` is the way to
  read a V-rule.

## `mode: partial` skips the planner, not the parser

A statement DataFusion cannot plan declares `mode: partial` in its YAML. What
that actually means:

| Still runs | Skipped |
|---|---|
| parsing, the full AST | DataFusion planning |
| E004, E005, E010, E011, E012 | E003's plan-derived output check |
| the rewrite step | — |

So `partial` is **not** an escape hatch from the rules. It is specifically "the
planner cannot derive this statement's output schema; the oracle will supply it
instead". The YAML makes the exception visible — it is never a silent fallback.

The common legitimate case is `MERGE`, which DataFusion cannot plan at all.

## The rewrite step

Before planning, constructs the planner cannot read are stripped — and each
removal **records what it stripped so a later pass can enforce the rule it stood
for**. That coupling is the point: removing the snapshot pin is what lets E006
check that the pin was there.

| Construct | Why |
|---|---|
| `AT (VERSION => $snapshot)` | DuckLake time travel; sqlparser 0.59 cannot parse `=>`. Recorded, then enforced by E006 |
| `name := value` named args | DataFusion's planner rejects named arguments; dropping the prefix makes them positional, which is all schema inference needs |
| `MERGE INTO` | split — the source query is planned, target assignments checked by name and type against the entity |
| `RETURNING` | removed; output schema comes from the entity's columns |
| `ON CONFLICT DO NOTHING` / `DO UPDATE SET` | removed after the assignment list is checked |
| `FOR UPDATE` / `SKIP LOCKED` / `NOWAIT` | removed; they do not affect schema |
| named params | rewritten to positional placeholders carrying their declared types |

If you add SQL using a construct the planner chokes on, the first question is
whether a rewrite belongs here rather than whether the statement needs
`partial`.

## The oracle: a real database has the final word

`tests/oracle.rs` builds each table from the **generated** lake DDL in a real
DuckDB and compares DuckDB's `DESCRIBE` against the entity declaration. The
analyser works in its own type space; the oracle compares type-name strings
against the engine that will actually run the query.

Two consequences worth internalising:

- A declaration can satisfy the analyser and still fail the oracle. The oracle
  is the authority on what the database returns.
- Aggregates are the usual culprit. DuckDB types `sum(BIGINT)` as `HUGEINT`, so
  wrap lake aggregates other than `count(*)` in an explicit
  `CAST(… AS BIGINT)`. Aggregate results are also plan-nullable: declare them
  nullable and `unwrap_or(0)` at the call site.

Result columns **default to nullable**, unlike params. Neither database reports
result nullability on its own, so non-null is an opt-in assertion the analyser
has to prove.

## ff-analysis is the planner, and it is a separate repo

This is the part nobody guesses, and the reason this skill exists.

`er-sql-check` does **not** depend on the standalone `sqlparser` crate. It
parses and plans only through
`ff-analysis = { git = "…/feather-flow", tag = … }`, which brings
**DataFusion 52 / arrow 57 / sqlparser 0.59**. The workspace's
`sqlparser = "0.63"` has exactly one consumer, and it is `dedupe-core`.

So: *"the repo uses sqlparser 0.63 for SQL static analysis"* is wrong, and the
arrow major here (57) deliberately differs from the duckdb-pulled 58 the rest of
the workspace uses. The split never unifies and is contained to this crate,
because the analyser works in `ff_analysis::SqlType` and the oracle compares
type-name strings.

**The workflow that follows:** DataFusion does not know DuckDB's functions, so
ff-analysis carries stubs for them — `to_json`, `map_from_entries`,
`json_keys`, `array_length`, `list`, window functions, `concat_ws`,
`element_at`, a custom planner for DuckDB `MAP {…}` literals and subscript
access, unicode normalizers, variadic regexp forms, array-literal planning.

If your SQL calls a DuckDB function the planner has never heard of, the fix is
**a stub in ff-analysis, in another repository, plus a tag bump here**. It is
not a change you can make in this repo alone. Full procedure:
[references/ff-analysis.md](references/ff-analysis.md).

Also: `datafusion-common` and `datafusion-expr` are pinned to the same major
ff-analysis resolves, so the `LogicalPlan` types unify. Bumping either alone
cannot go green.

## Declared statement, or generator?

Not everything can be a declared statement. The decision:

| Shape | Governance |
|---|---|
| Fixed SQL, known at authoring time | a declared `.sql` + `.yaml` — planned by the analyser |
| SQL whose **shape** depends on runtime config or a trained model | a Rust generator fn + a plan-vector test that plans every shape it can emit |
| `dedupe-core`'s model-derived blocking/scoring/clustering SQL | captured golden fixtures (an execution oracle), **never** the analyser |

A config- or tenant-driven statement is the middle row: write the generator
beside its domain crate, and add a vector test that plans its real output over
representative config fixtures. Do not try to force it into a declared
statement, and do not add analyser coverage to `dedupe-core` model SQL.

The rewrite rules, the idioms that plan and bind, and what the emitter
generates: [references/rewrites.md](references/rewrites.md).
