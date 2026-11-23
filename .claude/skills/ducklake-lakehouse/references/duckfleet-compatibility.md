# DuckFleet compatibility qualification — #1128 / #1169

Decision: **go for interchangeable execution of the qualified read and account
golden-task envelope on DuckDB 2.0.1**. This replaces #1128's engine-mismatch
no-go; it is not a production rollout or a claim that every pipeline path is
remote. Full API workflow, economics and remaining-path qualification remain
#1134 and #1170.

## Contents

- [Reproduce](#reproduce)
- [Assessed version matrix](#assessed-version-matrix)
- [What passed](#what-passed)
- [Adoption boundaries](#adoption-boundaries)

## Reproduce

From the repository root, with Docker and Python >= 3.11:

```sh
PYTHON=python3.12 make check-duckfleet-compat
PYTHON=python3.12 bash scripts/duckfleet/run-isolation.sh
make check-duckfleet-reads
make check-duckfleet-task
```

Each runner checks out the exact upstream revision embedded in its script,
creates unique Compose resources, runs the actual Rust worker/adapter, and
removes only its own containers/volumes. The Python packages are discovery-only;
the product and dbt runtime remain Rust. Do not point the clients at an existing
deployment. `artifacts/duckfleet/compatibility.json` is the machine-readable
result; a missing or failed probe prevents `go`.

## Assessed version matrix

| Component | ER baseline | Qualified DuckFleet |
|---|---|---|
| Source | `dba8b69f0175cbd786838cecae5c00e1576d5a6f` | `778dbe023203570f55e1265a91d4ec26885b250a` |
| DuckDB | 2.0.1 (`duckdb` crate `=1.20001.0`) | 2.0.1 (`duckdb` crate `=1.20001.0`) |
| Rust Arrow major | 60 | 60 |
| DuckLake catalog | 1.0, no automatic migration | 1.0, no automatic migration |
| Discovery decode | Python DuckDB 2.0.1, PyArrow 23.0.1 | HTTP Arrow IPC from the actual Rust worker |

The report records loaded extension versions and fixture SHA-256s. The baseline
wheel identifies the released engine but is not a substitute for the bundled
Rust build or `make check`.

## What passed

- All 58 distinct floating-point fixture values retain their exact bits through
  JSON binding and Arrow. BIGINT, quoted text, DECIMAL, date, microsecond
  timestamp, boolean and null values also preserve their Arrow types.
- Captured prediction stages 05–07 produce 1,841 results on both engines and
  match the existing golden tolerances. The observed cross-build maximum deltas
  were 0 for weight and `6.938893903907228e-18` for probability. Each backend is
  byte-identical on rerun; cross-build scoring is held to `1e-12`, not presented
  as bit-for-bit equivalence across builds.
- Fresh local and PostgreSQL DuckLake 1.0 catalogs preserve snapshot, commit,
  rollback and read-only behavior. Startup/probe attachments use
  `DUCKLAKE_VERSION '1.0'`; no `AUTOMATIC_MIGRATION` is enabled.
- The measured 2.0.1 expression contract passes: partial regex matching,
  factorial before multiplication (`3! * 2 = 12`), binary literals as `BLOB`,
  `preserve_identifier_case = preserve_case`, JSON extraction returning `NULL`,
  `/` by zero returning infinity, and `%` by zero raising an error. No legacy SQL
  compatibility bridge is enabled. pgwire nested/binary results require an
  explicit `VARCHAR` cast; the generated ER reads use scalar Arrow results.
- A 61-second statement deadline is accepted within the bounded task policy.
  Exactly 100,000 rows return successfully; 100,001 is refused explicitly rather
  than truncated. Interactive results also retain the 16-MiB bound; durable jobs
  carry larger results through scoped artifacts.
- Three independent Compose deployments reject every cross-dataset credential,
  write, history access and session read/delete. Catalog and object sentinels
  stay independent, and history contains no cross-dataset credentials.
- The generated read preserves filtered/keyset snapshots, visibility after a
  later update, typed nulls, temporary state, rollback, SQL-error recovery,
  cancellation and explicit close. The worker ends with zero sessions and active
  queries.
- The account golden task matches embedded output and rerun results: 4 golden
  records, 16 lineage rows, 3 matching scores and 10 correlated remote
  statements. Concurrent writers are excluded and an undersized placement fails.
- Both COMMIT faults reconcile correctly: a lost reply after the commit landed
  yields exactly one receipt and `committed`; an unsent COMMIT yields no receipt
  and `rolled_back`. No uncertain write is replayed.

Retained evidence is in `duckfleet-compatibility-evidence.json`,
`duckfleet-read-evidence.json`, `duckfleet-task-evidence.json` and
`security/references/duckfleet-isolation-evidence.json`.

## Adoption boundaries

The go decision covers the evidence above only. Production adoption still needs
#1134's customer-scoped API workflow, provision/upgrade/retention checks,
performance and cost measurements, monitoring, ownership and rollback rehearsal.
#1170 must mark every other pipeline/training/read path as remote with parity or
explicitly refused; no silent skip is allowed. AWS bucket policy, production
projected-secret inspection and production identity access remain rollout gates,
not facts established by Compose.

Catalog format and application schema have different owners. Keep automatic
catalog migration disabled and test any deliberate format change on a disposable
copy. A new worker binary or successful SQL request does not authorize a catalog
upgrade. Local proofs do not establish live AWS IAM, EKS NetworkPolicy, hostile
multitenancy, provider cost or every nested/extension type.

The earlier #1128 assessment failed floating-point transport, used different
engine/catalog versions and correctly returned no-go. Upstream PR #117 fixed the
float parser; #1168/#150 then put ER and DuckFleet on the same released engine.
This ticket re-measured the combination rather than carrying that decision
forward.
