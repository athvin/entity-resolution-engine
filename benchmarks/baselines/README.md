# Measured baselines

No measured CI baseline is currently committed. The smoke workflow runs and reports
`NO_BASELINE` until a comparable measurement is reviewed and written here.

The former `smoke.json` was synthetic unit-test data. Its fixture remains at
`tests/unit/bench/data/smoke_baseline.json`; it is not a performance measurement.

See [the performance guide](../../docs/performance.md#benchmark-baselines) for
creating a baseline. Commit measured `<scale>.json` files together with the matching
flags in `benchmarks/scales.yaml` and dispatch options in the benchmark workflow.
