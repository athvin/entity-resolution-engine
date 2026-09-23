# 10M full reload and 100K incremental benchmark

One unprofiled trial of the tuned application on 2026-09-23 completed both timed
workloads on the same DuckLake instance:

| Workload | Processing time | Records |
|---|---:|---:|
| Full reload: ingest, clean, train, match, reconcile, golden records and lineage | **1h 46m 16.63s** | 10,000,000 |
| Incremental: ingest, clean, match, reconcile, golden records and lineage | **2m 49.89s** | 100,000 added; 10,100,000 total |

Training took **1h 23m 18.69s**, or **78.39%** of the full reload. The incremental
workload reuses the trained model. This is a measured scale test, not a repeated
baseline/candidate comparison or a claim that the earlier 1M tuning gains hold at
10M. See the [1M tuning measurements](performance-workload-tuning.md) separately.

**Validation status: passed through snapshot recovery.** Both timed command chains
succeeded. The original campaign failed afterward in the benchmark's corpus-wide
candidate-pair `DISTINCT`, which exceeded the 4 GB DuckDB limit across 567,184,685 pairs.
The application's full load and incremental commands did not fail. The original
failed result is retained unchanged.

The [benchmark serializer](../benchmarks/semantic_outputs.py) now deduplicates
ordered ranges of integer record IDs and encodes JSON in pages. Six focused checks
verify unchanged legacy hashes, including overlapping keys, duplicates, nulls,
empty strings, Unicode and page boundaries. Recovery used the saved lake snapshots,
the same application image and the same resource limits. It repeated only deferred
validation and the frozen-model reference run, without retraining or replacing
the measured workload times. The
[recovery log](../artifacts/bench/workload-10m-100k-20260923T122937Z/recovery.log)
and [recovery result](../artifacts/bench/workload-10m-100k-20260923T122937Z/validation-recovery/result.json)
record that separate audit. Recovery completed successfully, including the
frozen-model incremental/full equivalence check. This does not overwrite the
original campaign's failed status or error.

## Stage times

The full reload invokes separate CLI commands. Incremental processing invokes
three ingest commands followed by one `run-all`, so its individual stages come
from the stage ledger. Ledger durations exclude CLI startup and do not sum to the
end-to-end command total.

| Stage | Full command time | Full ledger time | Incremental ledger time |
|---|---:|---:|---:|
| Ingest, all three sources | 42.665s | 35.215s | 7.561s |
| Clean and standardize | 107.381s | 105.050s | 56.343s |
| Train | 4,998.687s | 4,994.955s | model reused |
| Match | 751.126s | 747.426s | 58.352s |
| Reconcile entities | 317.685s | 313.903s | 18.585s |
| Golden records and lineage | 159.086s | 156.676s | 20.091s |

Incremental ingestion took 14.114s including command startup; its `run-all`
command took 155.775s. Reconciliation processed 243,391 affected records, with
72,399 touched entity IDs entering golden assembly and 70,651 output entities.
The full reload created 3,898,183 entities and stored 8,451,869 scored pairs,
of which 8,192,404 exceeded the auto-merge threshold. Candidate-pair counts and
ground-truth quality below were measured separately from workload timing.

## Outputs and quality

The full-load and incremental snapshot audits passed their record, membership,
golden-record and lineage checks. Model and frozen-TF content hashes are identical
before and after the increment and the full reference run. The reference's cluster
partition and all six canonical output hashes match the incremental snapshot:
standardized records, blocking keys, candidate pairs, golden records, lineage and
score classifications. Comparing all **8,621,617 scored pairs** found **zero
mismatches and a maximum probability difference of 0.0** (tolerance `1e-10`).
The reference used the same frozen model and TF snapshot, without retraining.

| Output or metric | After full reload | After 100K increment |
|---|---:|---:|
| Active records and memberships | 10,000,000 | 10,100,000 |
| Golden records | 3,898,183 | 3,916,446 |
| Golden lineage rows | 23,389,098 | 23,498,676 |
| Candidate pairs | 567,184,685 | 578,562,960 |
| Stored scored pairs | 8,451,869 | 8,621,617 |
| Blocking recall | 99.5622% | 99.5622% |
| Cluster precision | 82.7406% | 82.7336% |
| Cluster recall | 97.9097% | 97.9230% |
| False-positive cluster pairs | 1,633,894 | 1,668,579 |

The existing precision gap remains. Completing the pipeline and preserving
incremental semantics are distinct from meeting a production accuracy target.

At the same local resource envelope and application image, the earlier three
1M trials had medians of 91.70s for reload and 39.48s for a 100K increment. This
single 10M result took about 69.5 times as long for reload and 4.3 times as long for
the same-sized increment against the larger lake. These describe observed workload
scaling, not a controlled tuning gain or a prediction for other machines/corpora.

## Resource envelope and observations

| Measured resource | Full reload | Incremental |
|---|---:|---:|
| Pipeline CPU seconds | 6,676.304 | 266.740 |
| Sampled container memory peak | 9.37 GiB | 8.59 GiB |
| Sampled temporary spill peak | 141.48 GiB | 6.01 GiB |

The pipeline used two CPUs, two DuckDB threads, a 4 GB DuckDB memory limit and
a 10 GiB container limit on the local ARM64 Docker host. PostgreSQL and MinIO
provided the DuckLake catalog and object storage. Runtime pins were DuckDB 1.5.5,
DuckLake `d8a1881e`, Splink `5.0.0.dev5`, dbt-core 1.12.2 and dbt-duckdb 1.11.0.
Container memory includes
children and page cache and excludes the catalog/object-store services. Sampling
can miss brief peaks; these numbers are not minimum memory requirements. Spill is
the size of live temporary files, not cumulative bytes written.

The default uncapped training configuration was retained (`training.em.max_pairs:
null`). Email-based EM converged in six iterations; the broad surname/postcode
session converged in five. The long training duration and large spill are observed
facts. This timing run did not collect native query or Python profiles, so it does
not establish a new query-level explanation or justify an index/layout change.

## Corpus and reproducibility

The baseline generator used seed 42, 4,000,000 base personas and 10,000,000 base
records across CRM, billing and webforms. The `mixed-v1` increment adds 100,000
new record keys: 50,000 for existing people and 50,000 for 20,000 new people.
The baseline generator's known limitations still apply; this is not the hard
profile or real tenant data. A fresh lake does not imply cold OS caches.

Application image:
`sha256:3a89366a7d39286e75c4267263bc21ebe6424e3edbfe55b7516e1da9faa4499b`.
Source-content hash:
`0bc7697574a02b2da49a8fc198b6b3d98b5345e67491dda207a5d98c0351fb54`.
All 118 application/config/dbt files in its source manifest matched the checkout
before starting. The benchmark harness was frozen separately in the artifact
directory. Generation took 390.7s and is excluded from both processing times,
along with setup, checkpointing, validation and the final reference rescore.

Raw artifacts are retained under
[`artifacts/bench/workload-10m-100k-20260923T122937Z/`](../artifacts/bench/workload-10m-100k-20260923T122937Z/).
The [command ledger](../artifacts/bench/workload-10m-100k-20260923T122937Z/run-001/result.json),
[resource samples](../artifacts/bench/workload-10m-100k-20260923T122937Z/run-001/resources.jsonl)
and [execution log](../artifacts/bench/workload-10m-100k-20260923T122937Z/pass-001.log)
contain the underlying evidence. Raw artifacts are ignored by Git.
The repository also retains [machine-readable measurements](measurements/workload-10m-100k-20260923.json).

```sh
uv run python benchmarks/full_pipeline.py \
  --scale 10m --local --with-incremental \
  --incremental-records 100000 --incremental-scenario mixed-v1 \
  --image sha256:3a89366a7d39286e75c4267263bc21ebe6424e3edbfe55b7516e1da9faa4499b \
  --image-source 0bc7697574a02b2da49a8fc198b6b3d98b5345e67491dda207a5d98c0351fb54 \
  --keep-failed --out artifacts/bench/<new-output-directory>
```

`--local` derives limits from available host capacity. Repeated comparisons must
verify that the resulting CPU, container memory and DuckDB limits match this trial.
