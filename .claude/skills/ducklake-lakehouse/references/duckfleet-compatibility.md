# DuckFleet compatibility discovery — #1128

Decision: **no-go for adopting the assessed DuckFleet build as an interchangeable
ER execution backend**. Continue the epic's isolated implementation work; do not
point this build at a customer catalog. This is a discovery result, not a shipped
remote backend or a claim that the complete pipeline passed.

## Reproduce

From the repository root, with Docker and Python >= 3.11:

```sh
PYTHON=python3.12 make check-duckfleet-compat
```

The runner checks out the exact upstream revision embedded in
`scripts/duckfleet/run-compatibility.sh`, builds its worker, creates a unique
Compose project with fresh PostgreSQL/object-store volumes and a dynamically
allocated loopback port, and installs the pinned discovery-only Python packages
in a temporary venv. It removes only its own containers/volumes on exit.
The backend remains Rust; these Python packages are not runtime dependencies.

The report is `artifacts/duckfleet/compatibility.json`. **Exit 1 is the recorded
no-go**, not a skipped test. An absent probe, a failed comparison or a failed
session cleanup cannot produce `go`. Build/startup failures also exit nonzero;
check whether a new report was produced before interpreting an earlier file.
The negative gate tests run with the same dependencies:

```sh
python -m unittest discover -s scripts/duckfleet -p 'test_*.py'
```

Do not run the probe client against an existing deployment. Its explicit
`--disposable-compose` acknowledgement is required because it creates synthetic
lakes using upstream's local-only credentials. The wrapper owns resource cleanup;
the lower-level client deliberately leaves its unique synthetic catalogs behind
for inspection until the disposable stack is removed.

## Assessed version matrix

Historical assessment, not floating compatibility guidance:

| Component | Embedded baseline / discovery client | Assessed DuckFleet |
|---|---|---|
| Source | ER `99b0d1a9a8535f6800a681e43f62e31336b06eb8` | `672b3e3e82b61122c9647dd5dc85d32a3cedf09d` |
| Released DuckDB | 2.0.0 (`duckdb` Rust crate `=1.20000.1`) | 1.5.4 (`duckdb` Rust crate `=1.10504.0`) |
| Rust Arrow major | 60 | 58 |
| Discovery decoding | Python DuckDB 2.0.0, PyArrow 23.0.1 | HTTP Arrow IPC from the actual Rust worker |
| Catalogs tested | fresh local DuckLake metadata + local Parquet | fresh PostgreSQL metadata schema + S3-compatible Parquet |

The JSON report records loaded extension versions and fixture SHA-256s. The
upstream build scripts checksum-pin its engine and extension downloads. The
baseline wheel identifies the released engine but is **not** a substitute for
our bundled Rust build or `make check`.

The candidate to qualify next is DuckFleet built on **the same released engine
and compatible extensions as ER**, retaining ER's engine pin. No supported
interchangeable combination is declared by this experiment. Do not downgrade ER
or change a production catalog to make discovery pass. Arrow IPC interoperated
for the tested types despite different Rust Arrow majors; this does not establish
support for every nested or extension type.

The recorded [JSON evidence](duckfleet-compatibility-evidence.json) is a synthetic
fixture run from 2026-11-21 UTC. It is historical evidence; rerun after any pin or
protocol change.

## What the probes establish

- Bound BIGINT near the signed limit, quoted text, precise DECIMAL, date,
  microsecond timestamp, boolean and null values survive HTTP/Arrow unchanged.
- Temporary tables survive requests within a session and are absent from another
  session on the same worker.
- Explicit `METADATA_SCHEMA`, `DATA_PATH`, `AUTOMATIC_MIGRATION false` and disabled
  data inlining work on a fresh remote catalog. A native temporary relation can
  feed a dedicated lake-write transaction. A snapshot continues to see one row
  after a second is committed; rollback preserves two rows. Read-only reopen sees
  committed output and rejects writes.
- Captured prediction SQL stages 05–07, starting from the committed
  `blocked_with_cols.parquet`, produce 1,841 predictions on both engines. Both
  match the prediction golden's exact identifiers/comparison levels and existing
  numeric tolerances (weight 1e-8, probability 1e-10).
- Exact cross-engine equality fails: the observed maximum absolute differences
  were 7.105427357601002e-15 in weight and 2.220446049250313e-16 in probability.
  This is small enough for the historical oracle, but is not evidence of identical
  scores. Versions and host environments differ, so this experiment cannot assign
  the cause to the engine upgrade alone. Repeated execution in fresh sessions is
  checked separately from cross-engine equality.
- A 61-second deadline is rejected before execution; a 100,001-row response is
  rejected rather than truncated. These are measured API limits, not timing or
  capacity benchmarks. Source also bounds encoded results at 16 MiB, SQL text at
  262,144 bytes and parameters at 10,000 per request.

## Adoption conditions and ownership

1. **Version and scoring qualification.** Align the worker engine/extensions with
   ER, then repeat the harness on comparable runtime platforms and run actual
   bundled-Rust/executor parity. Investigate exact differences rather than
   loosening the gate to hide them. #1130 owns adapter parity; #1134 owns complete
   workflow qualification. Any upstream engine change needs its own DuckFleet PR
   and harness evidence.
2. **Catalog policy and isolation (#1129).** DuckFleet's startup attaches its
   deployment lake without `AUTOMATIC_MIGRATION false` or metadata-schema
   selection, then issues `USE lake`. The successful probe uses an additional,
   explicitly configured disposable attach; it does **not** fix startup policy.
   The ER backend must use a server-owned dataset binding, fully qualified lake
   references and native temporary intermediates. Configure the startup attach
   correctly before a customer catalog is opened; issuing policy SQL later is too
   late to prevent an automatic migration.
3. **Long-running execution (#1132).** Add an explicit bounded deadline policy
   for supported tasks. Admission of a larger deadline alone is insufficient:
   prove execution, cancellation and lifecycle behavior with the pilot.
4. **Transfers (#1131).** Keep intermediate tables remote and move large inputs
   and outputs through scoped object-store artifacts. The result cap can remain
   for interactive reads if the adapter gives an explicit bounded result policy;
   it is not permission to raise limits without memory accounting.
5. **Recovery and rollout (#1133/#1134).** Transaction conflict classification is
   not whole-job idempotency. Reconcile uncertain commits before replay or backend
   switching. Preserve the single writer per dataset and existing public contract.

Required extension capabilities remain governed by
`dedupe_core::session::BAKED_EXTENSIONS`: DuckLake, Parquet, JSON, ICU,
PostgreSQL catalog access and AWS/HTTPFS storage access. The matching functions in
these fixtures are built in. The local Compose credential test does not prove
production Pod Identity grants or credential refresh.

Catalog format and application schema have different owners. Keep automatic
catalog migration disabled; test any deliberate format change on a disposable
copy with a rollback/rebuild decision. Application table changes continue through
the entity catalog and its additive reconciler. Neither a new worker binary nor
a successful SQL request authorizes a format upgrade.

## Coverage limits

This experiment replays existing governed SQL; it does not replace generated
query functions, execute the entire task DAG, train a model, prove input delivery
semantics, validate hostile-tenant isolation, or measure deployment economics.
The fixed input contains synthetic data only. No production data, configuration,
credentials or infrastructure were changed for this assessment.
