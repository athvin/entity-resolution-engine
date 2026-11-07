# Declared SQL: should the framework own this statement?

`entity-framework` is the authority on **how** to declare a table or a
statement, and `sql-analysis` owns every E0xx remedy. This file is the reviewer's
much narrower question: **given a diff, is there SQL here the framework should
have owned, and did a schema decision get made that cannot be taken back?**

Most of this territory is gated hard, which makes the un-gated strip unusually
easy to name.

## Contents

- [The lake half is closed; the Postgres half is a convention](#the-lake-half-is-closed-the-postgres-half-is-a-convention)
- [Decisions a later migration cannot fix](#decisions-a-later-migration-cannot-fix)
- [The two-statement transaction](#the-two-statement-transaction)
- [Loading a catalog to write an artifact](#loading-a-catalog-to-write-an-artifact)
- [Already gated — do not raise](#already-gated--do-not-raise)

## The lake half is closed; the Postgres half is a convention

For **lake** SQL there is nothing left to review. `er-schema`'s posterity gates
assert that `er-server/src` names no `lake.main.<relation>` at all, that no query
string is assembled at runtime anywhere in it, that `Session::query_strings` has
no product callers outside the engine core, and that any qualified
`lake.main.<relation>` literal in the workspace names a declared relation. A
violation is a red test with the offending file in the message.

**Postgres is different, and this is the finding worth looking for.** Every
persistent control-plane statement is meant to be a declared `.sql` + `.yaml`
pair reached through a generated `er_entities::` function, and in practice it is
— but no scan enforces it. The shape to check a new call site against:

```sh
# The raw Postgres call sites that are legitimate. Session configuration and
# liveness probes, nothing that persists anything.
rg -n '\.query\(|\.execute\(|\.query_one\(|\.batch_execute\(' rust/crates/er-server/src/
```

What that returns today is `SET LOCAL`, savepoint management, a `pg_database`
existence probe, `set_config('er.actor', …)` and `SELECT 1`. A new raw statement
that **reads or writes a row of a declared relation** is the finding, and the
remedy is a declared statement, not a tidier string. The reason it matters more
here than tidiness: the generated function is where the type mapping, the
`RETURNING` list, the analyser's plan check and the oracle's type verdict all
attach. An inline string has none of them and will not be noticed when the
column changes under it.

## Decisions a later migration cannot fix

There are no migration files. The catalog is compiled into `er-server` and every
live store is reconciled to it at boot — additive changes applied, breaking ones
**refused**. So a schema decision in a diff is not "we can change it later" in
the usual sense, and two cases are worth a finding:

- **A closed-set column with no `accepted_values`.** The rule from the design is
  that closed sets get `accepted_values` on day one, because a CHECK constraint
  added later never lands on an existing table — the reconciler will not apply
  it. The column is then permanently unconstrained in every live store, while
  the YAML reads as though it has a vocabulary. `V11` forbids `accepted_values`
  on a *lake* entity (DuckLake enforces NOT NULL and nothing else); nothing
  requires it on a Postgres one, which is the gap.
- **A new `NOT NULL` column on a table that is already live.** It is Breaking
  for the reconciler, so the honest spelling is nullable, or nullable plus a
  backfill. This one *is* exercised — `er-entity-diff`'s Postgres tests run in
  the DuckLake e2e tier on every code PR — so raise it only when the diff adds
  the column and the e2e tier has not reported yet. Otherwise let the gate speak.

## The two-statement transaction

The framework has **no multi-statement form**, and `E012` rejects a
`WITH … INSERT` in a write folder. So a decision that archives one row and
inserts its successor is two declared files run inside one `client.transaction()`
on the per-request connection.

The failure a gate cannot see is the second half of that sentence. Two declared
statements issued on the client **without** a transaction compile, pass the
analyser, pass the oracle and leave a window where the archive landed and the
successor did not. Check that a new pair of writes that must be atomic is
actually inside a `transaction()`, and that the finish-first ordering is kept
where the design states one (`advance_segment` after `finish`, §3.7).

## Loading a catalog to write an artifact

`Catalog::load_dir` checks structural coherence only — it no longer enforces the
conventions. Anything about to **write a generated artifact** must load through
`er_entity_lint::load_linted` instead, or it emits from a catalog the lint
registry never saw. Nothing gates the choice; the crate's own module doc flags it
as a standing hazard "for every consumer that still calls `load_dir` directly",
which is a reviewer's job rather than a test's.

## Already gated — do not raise

- Any generated file edited by hand, or stale against its `entity.yaml` —
  `er-entity verify` regenerates every artifact and byte-compares.
- Any E0xx diagnostic: a missing `RETURNING` column, a wrong declaration order,
  an `UPDATE` without `updated_at = now()`, a multi-statement write. The
  analyser plans every declared statement in `make check`, and `sql-analysis`
  owns the remedies — quoting an E-code at an author who has not run the gate is
  not a review finding.
- A V-rule violation in an `entity.yaml` — `er-entity lint` runs first inside
  `verify`, and `er-entity lint --explain <code>` is the remedy. The reference
  is generated, so do not restate a rule from memory.
- A **new `mode: partial`**, which is the instinctive flag and is wrong here.
  `partial` skips the planner, not the parser; the AST passes still run, so it
  is not an escape hatch from the rules. It is the correct declaration for a
  statement DataFusion cannot plan, and `MERGE` is always partial.
- `dedupe-core`'s model-derived SQL, which is golden-governed and deliberately
  never analysed — see `er-matching`. The `__dedupe__` stage prefix is not
  cruft to clean up.
- `dedupe-cli`'s operator-named outputs, which are exempt by design.
