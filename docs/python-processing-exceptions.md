# Python processing exceptions

The normal data path keeps records and intermediate results in DuckDB. Python
selects SQL operations, manages transactions, invokes dbt/Splink, and reads small
results. This inventory covers production entry points and the benchmark/fixture
utilities audited for the SQL pushdown change. Pure collection APIs remain as
compatibility interfaces and independent test oracles; their presence does not
mean the CLI still uses them.

## Data flows now executed in DuckDB

| Use case | Previous Python boundary | Current implementation |
|---|---|---|
| CSV and common Parquet ingestion | Render every row, build payload dictionaries, normalize/hash, upload batches | `ingest/native.py` scans files, renders supported types, constructs JSON and hashes using `hashing.content_hash_sql`; `landing.py` retains the version-history anti-join |
| Frozen term frequencies | Fetch the complete lookup, build dictionaries, register them back into Splink | `matching/tf.py` creates local tables and registers their names with Splink |
| Incremental input selection | Fetch unscored keys, then upload them | `matching/incremental.py` materializes the key relation with CTAS |
| Score classification | Fetch every score, compare thresholds, decode review evidence | `matching/full.py:review_score_relation` aggregates counters and selects gray-band subjects in SQL |
| Review queue persistence | Fetch existing subjects and group their state in Python | `review/queue.py:upsert_subject_relation` joins existing state; settled subjects are skipped and open subjects refresh only their last-seen run |
| Scoring-generation guard | Fetch every current edge before counting generations | `entities/guards.py:assert_scoring_generation` returns only grouped generation counts |
| Affected-set discovery | Load score tuples, seed sets and membership dictionaries | `entities/relational_stage.py` materializes seed unions, assertion overrides, threshold neighbours and whole-entity membership joins, iterated to closure when cuts can change |
| Clustering input and output | Upload edge/node collections and fetch labels | `label_propagate_relations` consumes and returns local relations |
| Entity lifecycle mapping | Build overlap sets, offers, acceptances, assignments and transitions | `entities/relational.py` computes overlap aggregates, rankings and changes in SQL, for both initial and subsequent runs |
| Tombstone membership removal | Fetch absent keys, build sets, upload delete keys | A departed-record relation feeds the membership delete and removal-event grouping |
| Event deduplication | Read all existing event keys for the run | `entities/events.py` uses an anti-join at append time |
| Cut persistence | Read all active cut pairs to filter proposed cuts | `review/never_cut.py:persist_cuts` stages proposed cuts and anti-joins active cuts |
| Golden-record assembly | Build a touched-entity dictionary and retired-ID lists | `golden/assemble.py` writes the touched relation and reaps retired marts with SQL; the built-in no-op scorer returns before loading IDs |
| Assertion CSV imports | Read the complete file into objects; one transaction per assertion | `review/assertion_import.py` scans and validates natively, finds the first write conflict and bulk-commits the valid prefix in file order |
| Maintenance and model metadata | Fetch snapshots or versions for Python reduction | SQL selects the retention minimum and maximum numeric version; maintenance CALL results are counted in 1,024-row pages |
| Fixture TF loading/export | `executemany`, then `fetchall` plus `csv.writer` | `tests/helpers/model.py` uses `COPY FROM`; `scripts/regen_fixture_model.py` uses ordered `COPY TO` |
| Benchmark quality and comparisons | Python truth/edge sets and whole-output lists | The report uses `large_validation.quality_from_csv`; semantic comparison joins/sorts use SQL with bounded exact-format serialization |

Standardization, blocking, golden survivorship, TF materialization, score MERGE,
and stale-edge invalidation already executed in SQL. The nickname dbt seed already
uses the adapter's native bulk loader. There is no production unload CLI to convert.

## Retained Python work

### Exact event encoding and ID generation

**Code:** `entities/relational.py`, `entities/events.py`, `entities/ids.py`,
`lake/bulk.py`.

DuckDB determines group ownership and event membership sets. The existing event
encoder still validates and sorts details, stamps reasons, computes exact JSON
bytes/hashes, and mints event IDs in the established order. ID factories generate
only ordered IDs, in 1,024-row batches; they never receive membership rows.

Event encoding holds at most one page of 1,024 events at each batching boundary,
plus their payloads. A single event can contain an entire large component, so this
is a row bound, not a fixed byte bound. Canonical unreasoned hashes determine event
sort order; the final pass stamps reasons and IDs. Membership relations stay in SQL.

**Evidence:** DuckDB JSON and Python JSON differ on values such as `1e-6` and the
hexadecimal case in control-character escapes. Replacing the encoder would change
persisted `details_hash`, event ordering, and retry keys. Randomized lifecycle parity
tests compare the SQL plan with the pure planner; first-load persistence tests compare
complete entity, membership and event rows across batch boundaries.

**Revisit when:** an exact canonical encoder can reproduce all existing bytes and
hashes, or an explicitly versioned event-format migration is approved. A single
large event remains a memory constraint even with a native encoder.

### Assertion contradiction checks and deterministic graph cuts

**Code:** `review/assertions.py:check_contradiction_1`,
`review/never_cut.py:never_cut_fixpoint`, `entities/relational_stage.py`.

The active steward assertion set is loaded for the existing always-closure
contradiction check. After SQL clustering, only edges from components containing
a live never-match violation enter Python. All such components are handled together
so assertion ordering, shortest-path ties, probability/key cut ordering, protected
edges, escalation and global iteration counts retain their previous meaning.

**Bound:** the active assertion set plus the union of violating components. A
single giant violating component can still approach the entire affected graph;
there is no fixed memory cap. Ordinary components never enter Python.

**Evidence:** the existing graph tests and DuckLake cut/retraction integration tests
exercise these semantics. A generic SQL connected-components replacement would not
by itself implement the specified shortest-path and cut policy.

The existing label-propagation non-convergence diagnostic also reads the affected
labels and adjacency into Python on failure (`entities/cluster.py`). It reconstructs
true components to report the exact largest unsettled component. This exceptional
path is bounded by the affected graph, not a fixed page size, and writes no labels.

**Revisit when:** a native graph implementation has differential tests for all ties,
protected paths, multiple assertions and non-convergence, plus measurements on large
violating components. Do not process components independently without preserving the
global iteration and event-order contracts.

### File-rendering and parser compatibility

**Code:** `ingest/native.py`, `ingest/sources.py`, `ingest/landing.py`.

CSV header metadata is read in Python. Ordinary CSV and Parquet string, boolean,
integer, common date and timestamp columns remain native. Files with ambiguous
headers, unsupported Parquet renderings (including floats, decimals, timezone or
nested values), or native parse/key failures use the original adapter. This preserves
accepted dialects, source rendering, content hashes and file/row diagnostics.

**Bound:** one source file at a time; ingestion uploads at most 1,024 rows per batch.
Parquet's adapter has its own bounded fetch page. No lake rows are written until the
whole delivery is validated. The native/compatibility profiling spans identify the
path taken.

**Evidence:** parity tests cover Unicode/NFC, null and empty values, quotes, multiline
CSV, duplicate and case-colliding headers, typed Parquet, and malformed deliveries.
DuckDB's generic string casts are not equivalent to Python's `str`/`isoformat`.

**Revisit when:** additional explicit type renderings or CSV cases pass the same
reference tests. Never silently change content identity to remove a fallback.

### Assertion-import diagnostics and command output

**Code:** `review/assertion_import.py`, `review/assertions.py`.

Ordinary assertion input is native. A native scanner or validation rejection invokes
the original full-file parser to retain its permissive dialect and diagnostics.
This exceptional parser still holds the file in memory. The successful loader returns
a list of inserted assertions because its public API and CLI emit those rows; the
list is an output boundary, not processing that feeds rows back to the database.

Validation errors anywhere in the file write nothing. A later conflicting assertion
or self-pair commits the valid prefix, then uses the original writer to report the
same error. Duplicate same-kind rows preserve the first row's metadata and ID.

**Revisit when:** the parser can stream into temporary storage with identical error
precedence, and the public output API can move to an iterator without breaking callers.

### Corrupt score/review diagnostics and custom coherence scorers

**Code:** `matching/full.py`, `review/queue.py`, `golden/assemble.py`.

Malformed score evidence falls back to the original bounded score consumer to retain
validation and batch-prefix behavior. Duplicate open review subjects fetch only the
exceptional subject's rows for the established diagnostic. Valid scores/evidence stay
in DuckDB. Single-item steward review commands still return Python objects.

The exact built-in `NoopScorer` skips all entity reads. A custom scorer retains its
existing interface and may load the touched entity IDs or cluster records; its
implementation must define its own bounds. No custom production scorer is shipped.

**Revisit when:** adding a real scorer or changing diagnostic contracts. Prefer a
relation-based scoring interface for a new scorer.

### Model/configuration metadata and exact benchmark artifacts

**Code:** `matching/train.py`, `lake/model_registry.py`, `lake/maintain.py`,
`benchmarks/semantic_outputs.py`, fixture/test helpers.

Splink training returns learned parameter summaries, not the training corpus. Model
JSON, configuration, schema descriptions, counters and CLI manifests remain Python
objects. The version allocator reads one maximum; non-ASCII version digits use a
bounded compatibility reduction. Maintenance CALL result rows are read only to count
reported outcomes, at most 1,024 at a time.

Benchmark artifacts preserve the existing Python JSON encoding, lexically sorted
row hashes, candidate-pair JSON array hash and gzip JSON score array. SQL resolves
entity labels and sorts serialized rows; Python serializes/fetches 1,024-row pages.
No complete output or pair list is retained. Gzip container timestamps remain
non-deterministic as before; decompressed JSON is byte-identical.

Collection compatibility APIs also remain callable: `cluster.load_affected_set`,
`cluster.affected_edges`, `cluster_full`, `reconcile_plan` / `apply_reconcile_plan`,
the collection helpers in `entities/retraction.py`, `unscored_record_keys`,
`review_scored_pairs`, and both legacy touched-entity accessors in `obs/touched.py`
and `golden/assemble.py`. Their inputs or outputs can still be corpus-sized; the
normal CLI no longer uses them. The single-item review APIs and explicit incremental
`record_keys` argument also keep their public interfaces.

The pure reconciliation, threshold, quality, source-adapter and fixture-reader APIs
remain independent test oracles. Test invariant/replay helpers deliberately load
complete fixture results, and the fixed `base_10` benchmark check does the same.
Synthetic data generation is an external input producer and still owns its
persona/truth objects. These are not production DuckDB round trips.

**Revisit when:** artifact consumers accept a versioned Parquet/JSONL export, which
would permit direct `COPY TO`, or model metadata itself becomes large.

## COPY decisions

`COPY FROM` is used for the fixed-schema TF fixture; `COPY TO` exports its ordered
rows with an explicit CSV format. A test checks all 5,608 exported rows byte for byte
against the committed file. Native source scans still use `INSERT … SELECT` or CTAS
when projection, hashing, validation or deduplication is required. Score and membership
updates retain MERGE, and version-history writes retain their anti-joins.

`COPY` is a file import/export interface, not a blanket replacement for relational
transforms. DuckDB documents both native file loading and set-based inserts; the
performance problem is especially acute with individual row inserts. See the
[DuckDB COPY reference](https://duckdb.org/docs/current/sql/statements/copy) and
[INSERT guidance](https://duckdb.org/docs/current/data/insert).

Use `benchmarks/data_flows.py` to compare the retained reference paths with SQL at
identical input sizes, thread counts and memory limits. SQL avoids corpus-sized
Python collections, but total process RSS is not guaranteed to decrease: DuckDB
buffers, intermediate relations and parallel file readers also consume memory.

## Benchmark-only dense inference

`benchmarks/vector_runtime.py` reads distinct text inputs in batches of 64, uses
the baked tokenizer and CPU ONNX Runtime, then uploads band keys. Text deduplication,
record joins, composite keys, stoplist application and candidate evaluation remain
in DuckDB. This exception belongs to the optional benchmark image; no production
stage imports it. Model/runtime versions and hashes are checked before inference,
and runtime downloads are prohibited.

Memory includes ONNX and NumPy allocations outside DuckDB's memory limit. Batch
size bounds inference inputs; model weights and output width add a fixed footprint.
If measurements justify production inference, a separately reviewed exception and
cache contract are required. Revisit moving inference into DuckDB, potentially via
a Rust extension, only after profiling identifies it as a material hot path.
