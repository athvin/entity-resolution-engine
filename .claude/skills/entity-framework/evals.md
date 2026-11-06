# Evals: entity-framework

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Definition first

**Query:** "Add a `verified_at` timestamp to golden records."

Must:

- Edit `entities/golden_record/entity.yaml`, **not** the generated Rust,
  DDL, or TypeScript.
- Regenerate (`er-entity gen`) and then verify, and commit the definition and
  the regenerated artifacts **together**.
- Note that any `returns: entity` statement must now project the new column
  too (**E003**).

Credit for noting the lake has no defaults, so a new column must be nullable,
and that an added nullable column is **not** a version bump.

## 2. Never hand-edit generated output

**Query:** "The generated `er-entities` struct has the wrong field order and
it's breaking my code. Just reorder the fields in the generated file."

Must:

- Refuse: generated files are byte-compared by `er-entity verify`, so the edit
  is reverted by the next gate and fails CI in the meantime.
- Fix the ordering in the `entity.yaml` column list instead.

Credit for mentioning that files `gen` did not emit are reported as **orphans**,
which is the other half of the same gate.

## 3. Config-shaped SQL is not a statement

**Query:** "I need a read whose selected columns depend on which attributes the
tenant configured. Write the `.sql` and `.yaml` for it."

Must:

- Say it **cannot** be a declared statement, because the shape depends on
  runtime config.
- Route to a Rust generator function beside its domain crate plus a plan-vector
  test over real config fixtures.

Must not:

- Produce a statement with a placeholder column list, or suggest `SELECT *`
  (E011 has no override).

## 4. A lake read without a snapshot

**Query:** "Here's my new lake read — it selects from `lake.main.entities` with
a WHERE on `org_id`. Does it pass?"

Must:

- Flag **E006**: a lake read must carry `AT (VERSION => $snapshot)` unless it
  declares `pin: live` with a `pin_reason` — and a live read must then **not**
  carry a pin.
- Note that lake reads carry **no `org_id` predicate** in this design — tenant
  isolation is by catalog, so an `org_id` filter on a lake read is a sign of a
  misunderstanding (route to the security or ducklake-lakehouse skill).

## 5. check versus verify

**Query:** "Is `er-entity check` enough, or do I need `er-entity verify` too?"

Must:

- Say they are the **same code path** — aliases — so running one is running the
  other.
- Not describe `check` as a narrower staleness-only command.

Credit for listing what `verify` actually runs, in order, and for noting the
lint pass short-circuits on an error-severity finding.

## 6. A lake column change has two homes

**Query:** "Add a `confidence` double to `golden_records` on the lake."

Must:

- Edit **both** `entities/golden_record/entity.yaml` **and** the S5 registry in
  `rust/crates/er-schema/src/model.rs`, plus that crate's golden JSON dump.
- Give the reason: lake tables are created at runtime from the registry
  (`er_cli::create_ddl_table` over `all_specs()`), not from the entity-generated
  DDL, so an entity-only change regenerates cleanly, passes `er-entity verify`
  *and* `er-entity-diff`, then fails inside a run with
  `Binder Error: Referenced column "…" not found`.

Must not:

- Call the change complete because `er-entity verify` is green.
- Declare the column `nullable: false` (the lake has no defaults).

## 7. Selection: a stale generated artifact is not always this skill's

**Query:** "CI says the generated TypeScript types are stale. Fix it."

This scenario tests **selection**, not knowledge: the vocabulary ("generated",
"stale", "TypeScript") is this skill's, and the answer belongs to another.

Must:

- Load **api-contract** and regenerate `frontend/src/lib/api/schema.d.ts` from
  `openapi.json` via `openapi-typescript`.

Must not:

- Reach for `er-entity gen` on the strength of "generated TypeScript". That
  emits the entity bindings and readers under
  `frontend/src/lib/entities/`, which are a different artifact and a different
  gate, and leaves the OpenAPI types untouched.
