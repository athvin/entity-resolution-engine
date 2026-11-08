# Evals: ducklake-lakehouse

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

Where a scenario carries a **selection** must, record which skill actually
loaded — not just whether the answer was acceptable. An adjacent skill answering
well enough is a fail, because the content these scenarios exist to reach is
here.

## 1. The index that does not exist

**Query:** "Reads of `lake.main.golden_records` filtered by `org_id` are slow.
Add an index on that column."

Must:

- Say DuckLake has **no secondary indexes** — `CREATE INDEX` is rejected.
- Offer the two real levers (`partition_by`, `order_by`) and distinguish them:
  partitioning skips whole files, sorting tightens zone-maps within a file.
- Warn against partitioning on a high-cardinality column.
- **Selection:** load `ducklake-lakehouse` without being named. Answering from
  `ducklake-performance` alone is a fail — it is the right skill for whether to
  change layout, not for what the write-path levers are.

Credit for noting that layout should follow a measurement, and routing the
measurement to the ducklake-performance skill rather than guessing.

## 2. The s3:// credential shape

**Query:** "Set up a tenant whose lake lives at
`s3://er-dev-lake/tenants/acme/` and make sure the pod can read it."

Must:

- State that the path exists (#792) and give its shape correctly: an `s3://`
  `DATA_PATH` makes the attach emit a session-scoped `CREATE OR REPLACE
  SECRET`, and **no explicit key selects `PROVIDER CREDENTIAL_CHAIN`** — in
  the pod that is the Pod Identity role, which already carries the
  lake-bucket grants, so no secret material is set at all.
- Keep credentials out of the org row: the `ER_LAKE_S3_*` set is host-level
  process env, and the credential filter 422s credential-shaped keys in
  org/dataset env writes.
- Not invent `ER_S3_*` variables — those names stay banned — and not persist
  any key into the org's stored env.

Credit for noting what is still true: the catalog metadata is unchanged (a
Postgres DSN or file), and pointing a real tenant at `s3://` is the
operational step that retires the lake service user.

## 3. The metadata-schema naming trap

**Query:** "What does `ER_LAKE_METADATA_SCHEMA` control?"

Must:

- Say it sets the **user-table** schema — `DuckLakeConfig.schema`, giving
  `lake.<schema>.*`.
- Explicitly distinguish it from DuckLake's own `METADATA_SCHEMA` attach option
  (where DuckLake keeps its catalog tables), and name the *other* key that
  reaches it — `ER_LAKE_CATALOG_SCHEMA`, read into
  `DuckLakeConfig.metadata_schema`.

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
- **Selection:** load `ducklake-lakehouse` without being named. This is the
  scenario that catches the trigger gap — the `er-engine-ops` description also
  claims "lake maintenance", and it owns the `er` command surface rather than
  the ordering dependencies. Answering only from there is a fail.

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

## 6. Unpinning an attach option

**Query:** "Our deploy keeps failing on a catalog format mismatch. Let's drop
`AUTOMATIC_MIGRATION false` so DuckLake just migrates the catalog itself, and
while we're there turn data inlining back on for small writes."

Must:

- Refuse both as config tweaks and say what each pin buys: without
  `AUTOMATIC_MIGRATION false`, a newer DuckLake migrates a tenant's catalog in
  place, on open, as a side effect of a deploy — a catalog format change has to
  be a deliberate, reviewable event.
- Say `data_inlining_row_limit = 0` is what forces every row to Parquet, and
  that the golden fixtures depend on the byte-determinism that gives.

Must not:

- Treat either as a tuning knob, or propose flipping one to get a deploy green.

## 7. Every extension fetch 404s

**Query:** "Every DuckDB extension download is returning 404 — goldens, the
image bake, all of them. Is the community registry down?"

Must:

- Reach the engine **version label**, not the network: a `-dev` version routes
  the extension URL through `source_id` instead of the version tag, and no
  binary is published under a source hash, so everything fails at once.
- Point at `rust/.cargo/config.toml` as where the label is supplied by hand,
  because `libduckdb-sys`'s `bundled` build does not stamp one.

Must not:

- Diagnose it as an outage, a proxy or an egress problem on the strength of
  every fetch failing together.
