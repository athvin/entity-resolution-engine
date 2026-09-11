# Benchmark baselines

Each `<scale>.json` here is the committed performance baseline for one S10.2 scale. The
weekly benchmark (`.github/workflows/benchmark.yaml`) and any manual dispatch compare a
fresh run's per-phase `wall_ms` medians against the baseline for its scale and fail on a
regression. A scale is **dispatchable only once its baseline is committed** (S9.2), so the
workflow's dispatch `scale` options equal the set of files in this directory — a coupling
`benchmarks/report.py --validate-baselines` enforces on every PR (the `Baseline/dispatch
parity` step of `ci.yaml`'s static job).

## The envelope each baseline was measured under (S10.2)

A baseline carries a fingerprint recording the cgroup CPU quota, cgroup `memory.max`,
`ER_DUCKDB_THREADS` and `ER_DUCKDB_MEMORY_LIMIT` it ran under. `--write-baseline` **refuses
a `NON_COMPARABLE` run** — one whose fingerprint does not match its scale's row below, or
whose any per-phase `wall_ms` coefficient of variation exceeds 0.15 — because a baseline
taken on the wrong-shaped machine silently redefines every later comparison (S10.4). The
single source of truth for these values is `benchmarks/scales.yaml`; this table restates it
per committed baseline.

| scale | runner | cpu_limit | mem_limit | duckdb_memory_limit |
|---|---|---|---|---|
| smoke | ubuntu-latest | 2 | 6g | 4GB |
| 10k | ubuntu-latest | 2 | 6g | 4GB |
| 100k | ubuntu-latest-8-cores | 6 | 24g | 16GB |

`100k` is measured on the `ubuntu-latest-8-cores` larger runner: its 24g envelope does not
fit a 2-vCPU runner, and a `100k` run there is `NON_COMPARABLE`. That runner label is an
environmental precondition for (re)producing the `100k` baseline.

## Bootstrapping or refreshing a baseline

Dispatch `benchmark.yaml` at the scale, then freeze the measured run as its baseline:

```bash
uv run python benchmarks/report.py --write-baseline --scale 10k \
  --baselines-dir benchmarks/baselines --repeat 3
```

`--write-baseline` writes `<scale>.json` only if the run is comparable (`repeat >= 3`, the
fingerprint matches the scale's envelope, every phase CV `<= 0.15`). In CI the measured
run comes from the in-image `--run` step and the artifact it uploads; locally, a scale that
fits the host's envelope can be measured directly.

## Changing a baseline

A baseline changes **only through a reviewed pull request**. It is a gate input: an
unreviewed edit silently moves the regression ceiling for every subsequent run. Never
hand-edit the timings; re-measure and re-freeze with `--write-baseline`.

## S9.2 cadence divergence (pending spec review)

The board moves the weekly cron from `smoke` to `10k` once the `10k` baseline exists, and
this ticket (ER-102) does so. DesignDoc.md S9.2's Cadence sentence and the cron comment in
`benchmark.yaml` still read `smoke`; reconciling that wording is spec-amendment work and is
recorded here for the next spec review rather than changed under this ticket.
