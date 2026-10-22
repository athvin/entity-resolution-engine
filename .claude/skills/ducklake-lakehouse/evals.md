# Evals: ducklake-lakehouse

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. The index that does not exist

**Query:** "Reads of `lake.main.golden_records` filtered by `org_id` are slow.
Add an index on that column."

Must:

- Say DuckLake has **no secondary indexes** — `CREATE INDEX` is rejected.
- Offer the two real levers (`partition_by`, `order_by`) and distinguish them:
  partitioning skips whole files, sorting tightens zone-maps within a file.
- Warn against partitioning on a high-cardinality column.

Credit for noting that layout should follow a measurement, and routing the
measurement to the ducklake-performance skill rather than guessing.

## 2. The s3:// assumption

**Query:** "Set up a tenant whose lake lives at
`s3://er-dev-lake/tenants/acme/` and make sure the pod can read it."

Must:

- State that the Rust engine on `main` has **no object-store credential path** —
  no `httpfs`/`aws` load, no `CREATE SECRET`, no `ER_S3_*` reader — so an
  `s3://` lake cannot currently be reached.
- Not invent `ER_S3_*` variables or a `CREATE SECRET` call and present them as
  existing configuration.

Credit for identifying this as a missing feature with a design (credential
chain vs explicit key) rather than a setting someone forgot.

## 3. The metadata-schema naming trap

**Query:** "What does `ER_LAKE_METADATA_SCHEMA` control?"

Must:

- Say it sets the **user-table** schema — `DuckLakeConfig.schema`, giving
  `lake.<schema>.*`.
- Explicitly distinguish it from DuckLake's own `METADATA_SCHEMA` attach option
  (where DuckLake keeps its catalog tables), and note `DuckLakeConfig` on
  `main` has no field for that at all.

Must not:

- Infer from the name that it partitions DuckLake's catalog or isolates
  tenants' metadata.

## 4. Maintenance ordering

**Query:** "I want to reclaim disk on a tenant lake. I'll run merge-adjacent
first since compaction is the big win, then expire old snapshots."

Must:

- Correct the order: **flush inlined → expire → merge → rewrite → cleanup →
  delete orphans**, and explain at least one dependency (compaction cannot see
  inlined rows; cleanup finds nothing until snapshots are expired).
- Mention bounding compaction (`max_compacted_files` bounds memory).

Credit for warning that snapshot expiry must stay longer than the longest-lived
read cursor, since every lake read pins a snapshot.

## 5. Where intermediates go

**Query:** "I'm adding a staging table for a new pipeline stage. Should I
create it in the lake so other stages can read it?"

Must:

- Say intermediates belong in the **native temp catalog**, because DuckLake
  cannot host temp tables, and that only final outputs are written back.
- Note that a new *persistent* lake relation cannot be created ad hoc — it has
  to be declared (routing to the entity-framework skill).

Must not:

- Propose a hand-written `CREATE TABLE` against `lake.main.*`.
