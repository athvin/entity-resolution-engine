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
