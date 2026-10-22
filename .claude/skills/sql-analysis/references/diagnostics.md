# The E0xx diagnostics

Generated from source by `scripts/skills-check.sh regen` — the implemented set
is read out of `er-sql-check/src/analyze.rs` and the documented set out of
`docs/entity-framework-design.md`, so the "documented but not implemented" list
below is **derived, not maintained**. Do not hand-edit below the marker.

Invariant texts are clipped to a signpost. The design doc's table is the full
statement, and `er-sql-check`'s own source is the behaviour.

Remedies for the ones that fail most often are in the `rewrites.md` reference
beside this one, reached from the skill body; the entity-framework skill carries
the authoring rules that keep them from firing in the first place.

<!-- BEGIN GENERATED: diagnostics -->
The statement-level diagnostics, emitted by `er-sql-check`. These are a
separate family from the entity-level `V` rules in `er-entity-lint`.

**Implemented** (asserted against `analyze.rs`):

- `E001` — every table and column resolves in the declared store's catalog
- `E002` — every `$param` is declared in the yaml with a type compatible with the planner's inferred placeholder type; a lake statement may not declare a `list<varchar>` param (duckdb-rs has no LIST binding) …
- `E003` — the output equals the declaration — the named entity (`returns: entity`) or the typed column list (`returns: columns`): same columns, same order, compatible types, nullability no looser than …
- `E004` — every `NOT NULL` column without a `default` (or serial) is supplied — named columns as a set, a positional insert by arity; value compatibility is the planner's/oracle's half
- `E005` — every assigned column exists on the entity, and a `key` column is never assigned (identity is never rewritten); the immutables extend to the `natural_key` columns and `created_at` (written once, at …
- `E006` — the statement carries `AT (VERSION => $snapshot)` unless it declares `pin: live` with a `pin_reason` (the writer's own in-transaction reads); a live read must then NOT carry a pin, and `pin:` outside …
- `E009` — every qualified relation reference resolves to an entity of the statement's own store, and every entity beyond the statement's own is named under `reads:` (a `reads:` entry must itself name a …
- `E010` — a string literal meeting an `accepted_values` column — compared in a predicate, assigned, or inserted — is in the set
- `E011` — no `SELECT *` anywhere: projections, subqueries, CTEs, `INSERT ... SELECT`, `COPY TO`; every column is named, so an added column can never change a statement's shape
- `E012` — the statement's kind matches its folder, per the table in Statements and functions on disk
- `E014` — a mutation that rewrites a row assigns `updated_at`
- `E015` — the ON clause's target-side column set equals the declared `natural_key` (or `[id]` when none is declared) — the upsert identity lives in the entity file, not ad hoc per statement

**Documented but NOT implemented** — do not rely on these firing:

- `E007` — a `WHERE` clause is present on a control table
- `E008` — the source query plans and the target format is parquet or csv
- `E013` — every function the statement calls has a yaml under `functions/`, ours with a body or a built-in stub, with a signature the arguments satisfy

This gap is the stated reason `docs/entity-lint-rules.md` is generated
and byte-compared: a documented-only rule is exactly the drift that
produced it.
<!-- END GENERATED: diagnostics -->
