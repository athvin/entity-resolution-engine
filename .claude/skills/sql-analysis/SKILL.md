---
name: sql-analysis
description: Explains the static SQL analyser (`er-sql-check`) that plans every declared statement in this repo — the E0xx diagnostics and their remedies, the rewrite step, what `mode: partial` skips, the two-store catalog, the execution oracle, and the ff-analysis (feather-flow) dependency supplying the planner and its DuckDB function stubs. Use when an E0xx diagnostic fails, when a statement will not plan, when adding SQL that calls a DuckDB function the analyser does not know, when the oracle disagrees with a declared type, or when deciding between a declared statement and a plan-vector-tested generator. This skill owns the E0xx remedies and the oracle's verdict; for writing the entity or statement YAML itself see entity-framework; for `dedupe-core`'s model-derived SQL, which is golden-governed and never analysed, see er-matching.
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

- **Some documented codes are not implemented, and you must not rely on them
  firing.** Which ones is a set difference the reference derives — do not work
  from a remembered list, because a code that ships moves between the two halves
  with nothing else in the tree changing. (That drift is the stated reason
  `.claude/skills/entity-framework/references/entity-lint-rules.md` is generated and byte-compared.)
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
| the rewrite step, then parsing | DataFusion planning |
| **every AST pass**, E014 and E015 included | E003's plan-derived output check |
| E002's undeclared-`$param` check, by text | E002's declared-type-vs-inferred half — no plan, no inference |

So `partial` is **not** an escape hatch from the rules. It is specifically "the
planner cannot derive this statement's output schema; the oracle will supply it
instead". The YAML makes the exception visible — it is never a silent fallback.

The common legitimate case is `MERGE`, which DataFusion cannot plan at all — and
that is exactly why the AST half matters. E014 (`updated_at` is assigned) and
E015 (the `ON` clause matches the `natural_key`) have **no other subject**: a
MERGE is always partial, so if `partial` skipped the AST passes those two rules
could never fire at all. Read the "still runs" column as the whole AST pass set
rather than a list, because the set grows.

One real narrowing, and it is the only one: a partial statement that will not
parse *even after the rewrite* falls back to text heuristics — a `SELECT *`
regex and a leading-verb check, nothing else. That is the weakest check in the
system and the actual cost of exotic SQL.

## The rewrite step

Before planning, constructs the planner cannot read are stripped — and each
removal **records what it stripped so a later pass can enforce the rule it stood
for**. That coupling is the point, and these are the three removals where it is
load-bearing:

| Construct | Rewrite | Rule it preserves |
|---|---|---|
| `AT (VERSION => $snapshot)` | stripped, the snapshot param recorded | **E006** — removing the pin is what lets E006 check the pin was there |
| `MERGE INTO` | split: source query planned, target assignments checked by name and type against the entity | **E005**, **E014**, **E015** |
| named params | rewritten to positional placeholders carrying their declared types | **E002** |

[references/rewrites.md](references/rewrites.md) is the canonical list and
carries the rest — `RETURNING`, `ON CONFLICT`, the locking clauses,
named-argument prefixes — each with the rule it preserves or an explicit "none".

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
parses and plans only through `ff-analysis`, pinned by tag to the first-party
feather-flow repository. So *"the repo uses the workspace's sqlparser for SQL
static analysis"* is wrong: `ff_analysis::sqlparser` is
`pub use datafusion_expr::sqlparser`, and the workspace's own `sqlparser` has
exactly one consumer — `dedupe-core`.

That re-export is also the trap. Three sqlparser majors resolve in this build
graph, and `cargo tree` shows ff-analysis's **own** direct dependency, not the
one that actually parses. The pins, and which row is the parser, are derived
from `Cargo.lock` in [references/ff-analysis.md](references/ff-analysis.md) —
read them there rather than from memory.

The arrow major on the analyser side deliberately differs from the one the
`duckdb` crate pulls for the rest of the workspace. The split never unifies and
is contained to this crate, because the analyser works in `ff_analysis::SqlType`
and the oracle compares type-name strings.

**The workflow that follows:** DataFusion does not know DuckDB's function
library, so ff-analysis carries stubs for the ones this engine's SQL calls. What
is covered sits in the reference, tag by tag — the set grows with every
feather-flow release, so check it rather than assuming a function is missing.

If your SQL calls a DuckDB function the planner has never heard of, the fix is
**a stub in ff-analysis, in another repository, plus a tag bump here**. It is
not a change you can make in this repo alone.

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
