# Physical layout and lake maintenance

## Contents

- Choosing between partitioning and sort-on-write
- What DuckLake does not enforce
- The maintenance operations
- The ordering, and why it is an ordering
- Snapshot expiry is coupled to how you read
- The ICU gotcha in `expire_snapshots`
- Inspecting a lake

## Choosing between partitioning and sort-on-write

The two levers `LakeWriteOptions` carries, what each does, the high-cardinality
failure mode and the `order_by`-when-unsure default are in the SKILL body. What
it does not carry:

- **Over-partitioning costs twice.** Per-file overhead — open, read footer,
  plan — swamps the scan the layout saved, *and* many small files make every
  subsequent maintenance pass more expensive. The second cost is the one people
  do not budget for.
- **`order_by` pays only write-time sort effort**, and tightens the min/max
  zone-maps in the Parquet footers so a reader skips row groups *within* a
  file. That is why it is safe at any cardinality and helps range scans as well
  as equality, where partitioning only skips whole subtrees.
- **The test before reaching for `partition_by`:** you can name the query that
  filters on the column, and its cardinality is genuinely bounded. A column
  downstream merely *selects* does not qualify.

## What DuckLake does not enforce

That there are no secondary indexes and no primary-key or unique constraints is
in the SKILL body. What follows from it is the part worth carrying here: logical
keys are enforced by writers and by tests, which is the stated reason the repo
has posterity gates about declared relations rather than relying on the store.
So a duplicate-row bug is always a writer bug — there is no constraint that was
"supposed to" catch it, and no layout change that will.

## The maintenance operations

Exposed on `Session`, and surfaced by the CLI:

| Operation | What it does |
|---|---|
| `flush_inlined_data` | moves catalog-resident (inlined) rows out into Parquet data files |
| `merge_adjacent_files(max_file_size, max_compacted_files)` | tiered compaction of small adjacent files, without expiring snapshots |
| `expire_snapshots(older_than)` | drops snapshot history older than an interval |
| `rewrite_data_files(table)` | rewrites files whose deleted fraction exceeds the catalog's `rewrite_delete_threshold`, reclaiming space after heavy deletes/updates |
| cleanup / delete-orphans | removes files unreferenced by live snapshots, and files present in the data path but unknown to the catalog |

Both bounds on `merge_adjacent_files` exist to keep the work finite:
`max_file_size` caps output size, `max_compacted_files` caps files per table and
therefore **bounds memory**. An unbounded compaction over a large table is the
easy way to OOM a maintenance run.

Catalog options (`expire_older_than`, `delete_older_than`,
`target_file_size`, and arbitrary `KEY=VALUE`) can be set before a run, and the
full set is printable.

## The ordering, and why it is an ordering

A full checkpoint runs:

```
flush inlined → expire → merge → rewrite → cleanup → delete orphans
```

It is a sequence, not a menu, and each step depends on the one before:

- **Flush first.** Inlined rows are not files yet, so compaction cannot see
  them. Compacting before flushing leaves the inlined rows behind to be
  flushed as new small files afterwards.
- **Expire before cleanup.** A file is only unreferenced once no live snapshot
  points at it. Cleanup before expiry finds nothing to delete.
- **Delete orphans last.** It compares the data path against the catalog, so it
  must run after everything that writes or removes files, or it will classify
  in-progress output as an orphan.

Running the steps individually is supported and useful for bounded maintenance
windows — but if you run them out of order, expect to have achieved less than
the step names suggest.

## Snapshot expiry is coupled to how you read

Every lake read statement in this repo pins a snapshot (`AT (VERSION =>
$snapshot)`) — that is diagnostic **E006** in the entity framework. A pinned
snapshot is a reference to a point in history.

So snapshot expiry is not purely a housekeeping knob: **keep the history window
longer than your longest-lived cursor.** A paginated read that pins a snapshot
and then comes back for page 2 after the window closed will find its snapshot
gone. The same reasoning applies to a long-running transaction, which is why
`expire_snapshots`'s own doc comment says to keep the interval well above it.

The symptom of getting this wrong is an intermittent failure on a long or
resumed read, which is easy to misattribute to the reader.

## The ICU gotcha in `expire_snapshots`

The interval arithmetic is written as:

```sql
CALL ducklake_expire_snapshots('<alias>',
  older_than => now()::TIMESTAMP - INTERVAL '<interval>');
```

The `::TIMESTAMP` cast is required, not stylistic. The bundled DuckDB omits
ICU, so `now()` — which is `TIMESTAMP WITH TIME ZONE` — has no `- INTERVAL`
binding available. Casting to a zone-less `TIMESTAMP` finds the binding.

If you write new interval arithmetic against this build and get a "no function
matches" error, this is why. It is a property of the bundled build, so it
applies everywhere the engine runs, not just in tests.

## Inspecting a lake

For ad-hoc inspection, a `duckdb` shell with the `ducklake` extension attaches
the same catalog read-only — the same ATTACH the engine issues:

```sh
duckdb -c "INSTALL ducklake; LOAD ducklake; \
  ATTACH 'ducklake:<catalog>' AS lake (DATA_PATH '<data path>', READ_ONLY); \
  SELECT id, status, mode FROM lake.main.runs;"
```

Match the DuckDB version to the engine's pin where it matters — the pin is an
`=` pin, and the current value is in the rust-conventions skill's generated
crate map.

Two `Session` helpers exist for catalog introspection and are deliberately
exempt from the declared-statement rule, because they take a relation name
rather than being a fixed query: `lake_table_exists` and `relation_row_count`.
Prefer them over hand-written introspection SQL.

The supported way to browse actual data is the read API and the UI, not a
shell — the shell bypasses the snapshot pinning and role checks the product
surfaces apply.
