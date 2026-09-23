# Million-record full-load confirmation

On 2026-09-23, a fresh one-million-record load completed in **91.82 seconds** with
the experimental `training.em.max_pairs: 1000000` setting. The sequential uncapped
control took **97.13 seconds**. Training took **7.43 seconds versus 10.53 seconds**,
a **29.4% reduction**; the complete initial load took **5.5% less time**.

This is one comparison on a tuning corpus, not a repeated performance estimate.
The requested single-load check replaced the longer repetition campaign. No
interrupted run contributes to these timings. Production defaults remain unchanged
pending the [10M promotion gates](performance-experiments.md#acceptance).

| Stage | Uncapped EM, seconds | 1M EM target, seconds |
|---|---:|---:|
| Ingestion | 10.91 | 10.89 |
| Cleaning / standardization | 21.23 | 21.81 |
| Training | 10.53 | 7.43 |
| Matching | 13.07 | 13.23 |
| Entity reconciliation | 28.12 | 25.56 |
| Golden-record assembly | 13.27 | 12.89 |
| **Full load** | **97.13** | **91.82** |

The stage differences outside training include ordinary single-run variation;
they are not evidence of separate optimizations. Processing includes CLI startup
and all six stages. Synthetic-data generation, environment setup, validation and
teardown are excluded. The candidate's generation/setup took 62.88 seconds and its
worker, including that setup and validation, took 167.03 seconds.

## Output and quality

The candidate contains **1,000,000 standardized records and memberships**,
**397,753 golden records** and **2,386,518 lineage decisions**. Validation found no
missing memberships, missing/duplicate golden attributes, orphan golden records,
or lineage winners outside their entities. Every processing command exited zero.

| Cluster metric | Uncapped EM | 1M EM target |
|---|---:|---:|
| True-positive pairs | 710,811 | 710,824 |
| False-positive pairs | 229,065 | 228,989 |
| False-negative pairs | 89,189 | 89,176 |
| Precision | 75.62817% | 75.63462% |
| Recall | 88.851375% | 88.853000% |

Blocking is unchanged at 5,408,610 candidate pairs and 94.90525% recall. All quality
families use the shared `er.eval` implementation. The small improvement on this
synthetic corpus does not establish production accuracy.

Pipeline CPU consumption was 133.01 seconds uncapped and 122.41 seconds capped.
Sampled peak memory was 3.98 GiB and 4.03 GiB respectively; neither run showed
sampled DuckDB spill. These are observed peaks, not minimum memory requirements.

## Reproduction and evidence

Both runs use `hard-v1`, generator seed 20260101 and the reference configuration,
with only `training.em.max_pairs` changed between arms. Base CSV hashes match.
Each starts a new Postgres/MinIO/DuckLake stack on the same frozen application
image, limited to two CPUs, 10 GiB container memory and a 4 GB DuckDB limit.
Integration workers had finished before measurement; no other pipeline workload
ran concurrently. The pre-existing idle audit services were preserved.

The [measurement JSON](measurements/training-full-load-20260923.json) records the
immutable image/source hashes, configuration and input hashes, stage timings,
resource samples, quality counts, validation counts and initial scoring snapshots.
The source commit before these changes was `234e165`.

To run one initial load with the same experimental setting:

```sh
uv run python - <<'PY'
from pathlib import Path
import yaml
cfg = yaml.safe_load(Path('configs/default.yaml').read_text())
cfg['generator']['seed'] = 20260101
cfg['training']['em']['max_pairs'] = 1000000
Path('/tmp/er-training-1m.yaml').write_text(yaml.safe_dump(cfg))
PY
uv run python benchmarks/full_pipeline.py --scale 1m --local --repeat 1 \
  --config /tmp/er-training-1m.yaml --generator-profile hard-v1 \
  --out artifacts/performance/training-1m-full-load
```

Before the final run, static checks and 909 unit tests passed. All 347 regular
integration cases have passing results, including reruns of three initially
failing cases after repairing no-op exit handling and the full-mode expectation.
The optional slow fixture-model regeneration test was excluded.
