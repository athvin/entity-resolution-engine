# Splink 4 migration oracle

Captured from source `fc5070e`, Splink 4.0.16 and DuckDB 1.5.5, with two DuckDB
threads. The model is the old, pinned 10,000-record fixture model. Its blocking
rules are replaced by `1=1` for this test only, so all 276 pairs are compared,
including weak, negative, null and term-frequency-adjusted evidence. The original
model parameters and prior are unchanged. `inputs.json` freezes the records and TF
rows independently of future fixture regeneration.

`test_runtime.py` compares probabilities within an absolute 1e-10, comparison
vectors exactly, log2 evidence against the old Bayes factors, and review/merge
decisions exactly. The production blocking rules are covered separately by the
blocking parity integration tests. It also exercises table/Parquet cleanup and
1×1, 2×2 and 4×4 prediction chunks.

Reproduce using an image built from the baseline commit, then compare the generated
files under `artifacts/splink4-oracle/oracle/` with this directory:

```sh
mkdir -p artifacts/splink4-oracle
cp tests/fixtures/splink4/capture.py artifacts/splink4-oracle/
docker run --rm --network none \
  -v "$PWD/artifacts/splink4-oracle:/capture" \
  er-pipeline:splink4-baseline-fc5070e python /capture/capture.py
```
