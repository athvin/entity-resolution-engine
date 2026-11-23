# DuckFleet compatibility discovery — #1128

Decision: **go** (#1169, 2026-11-23) on the probe surface below. DuckFleet
`778dbe02` and ER both run DuckDB 2.0.1 with the same extension builds, and a
same-platform baseline matches the remote worker bit for bit. This is still a
discovery result over governed SQL replay. It is not a shipped remote backend,
whole-pipeline qualification (#1170) or a rollout decision (#1134). Do not point
a customer catalog at it on this evidence alone.

The original #1128 assessment (2026-11-21) was **no-go**, on engine mismatch
(DuckFleet 1.5.4 vs ER 2.0.0) and a one-ULP scoring difference. #1169 found
that difference was the baseline's platform: macOS libm against Linux workers.
The table below keeps both runs.

## Contents

- [Reproduce](#reproduce)
- [Assessed version matrix](#assessed-version-matrix)
- [What the probes establish](#what-the-probes-establish)
- [Adoption conditions and ownership](#adoption-conditions-and-ownership)
- [Coverage limits](#coverage-limits)

## Reproduce

From the repository root, with Docker and Python >= 3.11:

```sh
PYTHON=python3.12 make check-duckfleet-compat
```

The runner checks out the exact upstream revision embedded in
`scripts/duckfleet/run-compatibility.sh`, builds its worker, creates a unique
Compose project with fresh PostgreSQL/object-store volumes and a dynamically
allocated loopback port, and runs the probe client in a digest-pinned
`python:3.12-slim` container on the Compose network, with the pinned
discovery-only Python packages. The client never runs on the host, because scores
are compared bit-for-bit and libm differs between operating systems. The
`baseline_platform_matches_remote` probe refuses a baseline whose DuckDB
platform differs from the worker's. The runner removes only its own
containers/volumes on exit.
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

Recorded assessments, not floating compatibility guidance:

| Component | #1128 (2026-11-21): ER / DuckFleet | #1169 (2026-11-23): ER / DuckFleet |
|---|---|---|
| Source | ER `99b0d1a9` / `672b3e3e` | ER after #1171 / `778dbe023203570f55e1265a91d4ec26885b250a` |
| Released DuckDB | 2.0.0 (`=1.20000.1`) / 1.5.4 (`=1.10504.0`) | 2.0.1 (`=1.20001.0`) / 2.0.1 (`=1.20001.0`) |
| Rust Arrow major | 60 / 58 | 60 / 60 |
| Discovery baseline | Python DuckDB 2.0.0 on the macOS host | Python DuckDB 2.0.1 in linux_arm64, glibc 2.41 |
| Remote extensions | — | aws `9ba41b09a0`, ducklake `08089b16dc`, httpfs `f71cfdb4bf`, postgres_scanner `8b9d0bbe8d`, core v2.0.1 |
| Decision | no-go | **go**, 12/12 probes |

Both runs tested fresh local DuckLake metadata and local Parquet on the baseline
side, and a fresh PostgreSQL metadata schema with S3-compatible Parquet on the
remote side.

The JSON report records loaded extension versions and fixture SHA-256s. The
upstream build scripts checksum-pin its engine and extension downloads. The
baseline wheel identifies the released engine but is **not** a substitute for
our bundled Rust build or `make check`.

#1128 required DuckFleet built on **the same released engine and compatible
extensions as ER**. #1169 meets that with 2.0.1 on both sides. That is the only
combination this probe supports; any pin move on either side means rerunning it. Do not downgrade ER
or change a production catalog to make discovery pass. Arrow IPC interoperated
for the tested types despite different Rust Arrow majors; this does not establish
support for every nested or extension type.

The recorded `duckfleet-compatibility-evidence.json` is the synthetic
fixture run from #1169 (2026-11-23 UTC). It is historical evidence; rerun after any pin or
protocol change.

## What the probes establish

- Bound BIGINT near the signed limit, quoted text, precise DECIMAL, date,
  microsecond timestamp, boolean and null values survive HTTP/Arrow unchanged.
- Floating-point parameters round-trip bit-exactly: all 58 distinct fixture
  values (`bound_float_roundtrip`). Under #1128 JSON DOUBLE binding lost bits
  (`0.0012033694344163659` sent, `0.001203369434416366` returned); that probe
  now passes against `778dbe02`.
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
- Exact equality holds from a same-platform baseline. Stages 05–07 give 1,841
  predictions with identical SHA-256, zero weight/probability delta and an exact
  fresh-session rerun. Under #1128 the maximum differences were 7.1e-15 in
  weight and 2.2e-16 in probability (one ULP). #1169 reproduced one ULP from a
  macOS 2.0.1 baseline against the Linux 2.0.1 worker, then zero from linux_arm64:
  the cause is the baseline's libm, not the engine or DuckFleet.
- A deadline above 60 seconds is admitted (`long_task_deadline_admission`).
- Interactive results stay bounded (`bounded_result_policy`). 100,000 rows are
  returned whole; 100,001 are refused with `retry_safe: false`, never truncated.
  Bulk data travels through scoped object-store artifacts (#1131). Under #1128
  this probe was `bulk_result_capacity`, which expected the over-cap result to
  succeed. That expectation predates the decision to keep the cap. Source also
  bounds encoded results at 16 MiB, SQL text at 262,144 bytes and parameters at
  10,000 per request.

## Adoption conditions and ownership

1. **Version and scoring qualification — met by #1169.** The float round-trip,
   engine/extension alignment, same-platform baseline and bit-exact scoring all
   pass. The Rust executor parity is `make check-duckfleet-task`
   (`embedded_remote_equal`). The original condition follows.
   Preserve floating-point parameters
   across JSON decoding/binding first (a DuckFleet decoder change or an explicitly
   typed, lossless parameter representation must pass the exact round-trip probe).
   #1130 owns that adapter/upstream contract. Align the worker engine/extensions with
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
