# Performance experiments

The implementation first makes the comparisons trustworthy: production prediction
rules come from the active configuration, a full rescore retires obsolete active
pairs atomically, never cuts are rederived from the current graph, and `er correct`
refreshes TF with a resumable journal. Initial loads, incremental deliveries and
corrections are separate workloads. The existing defaults remain unchanged.
The [screening results](performance-screening.md) record the first million-record
quality comparisons and provider smoke test.
The [single full-load confirmation](performance-training-full-load.md) records
29.4% less training time and 5.5% less total processing time with the 1M EM target.

The historical 10M result spends about 80% of initial-load time in training. Its
491M-pair surname/postcode EM rule is distinct from the prediction `name_postal`
rule. Removing a prediction rule cannot remove that training cost. See the
[historical measurements](performance-splink5.md) for the exact image, corpus,
configuration and resource envelope; those timings are not a new baseline.

## Reproduce the campaign

Build the current source once, resolve its immutable image ID, and run trials
sequentially on an otherwise idle Docker host:

```sh
docker compose -f docker/compose.yaml --profile test build pipeline
ER_PERF_IMAGE=$(docker image inspect er-pipeline:ci --format '{{.Id}}')
uv run python benchmarks/performance_campaign.py \
  --image "$ER_PERF_IMAGE" --scale 1m --local --repeat 3 \
  --config configs/default.yaml \
  --profiles baseline hard-v1 --seeds 20260102 20260103 \
  --out artifacts/performance/screening
```

Arms are `current`, `em_1m`, `em_5m`, `em_10m`, `without_name_postal` and
`narrow_name_postal`. Splink's `max_pairs` is an approximate sampling target;
inspect actual sampled counts in the training logs. The narrow rule adds the given
name's first character.
`--arms` selects a subset and must include `current`. Each repetition starts a new
lake, trains explicitly, measures the initial load, applies a delivery using that
model, and runs a correction without retraining. Arm order reverses every other
repetition. Corpus caches include seed, profile, shape and source mappings in their
identity; inputs are immutable across arms.

Use a separate tuning seed, such as 20260101, before running the held-out seeds
above. After screening, rerun qualifying arms with `--scale 10m`, three repetitions,
and a new output directory. At 10M the incremental delivery has 100,000 records.
A full uncapped initial load can spill over 135 GiB; retain enough Docker disk space.
The local resource profile preserves corpus size and records its actual limits;
it cannot be compared with a preset-runner baseline.

Each trial keeps `results.json`, stage/resource logs, input hashes, image identity,
per-key block statistics and quality counts. `campaign.json` keeps paired datasets,
workload medians and individual gate verdicts. Setup, generation and validation are
excluded from processing time; training is included in the initial load. Failed
trials remain failed artifacts, never successful timing samples.

## Acceptance

Evaluate every held-out dataset independently. Cluster precision cannot decrease.
The maximum cluster pairwise recall loss is **0.01 percentage points** (0.0001 as
a fraction); spending any recall requires at least **20% end-to-end time reduction**
in the target workload. Require three 10M measurements and no repeatable slowdown
in another workload. The campaign flags a slowdown when every corresponding trial
in at least three repeats is slower. Inspect variance and resource comparability
before promotion; the report does not change production configuration.

These label-quality budgets do not relax exact incremental/full equivalence,
pure scoring or deterministic clustering. Report blocking, edge and cluster metrics
through the shared `er.eval` implementation. More retrieved pairs can lower cluster
precision through transitive false merges.

`hard-v1` adds independent spelling/missing-field errors and correlated contact
changes; rates and seed are recorded in `profile.json`. Movers deliberately reuse
a different persona's contacts, and a separate household axis shares contacts and
address across people. This is a stress profile, not a measured tenant distribution.
Exact email/phone agreement supplies a weak-positive evaluation slice, not free
labels: use negative and ambiguous household cases and steward validation too.
No real tenant corpus is bundled with this repository.

## Candidate-key experiments

The SQL-only provider computes trigram MinHash over distinct standardized name and
address strings. Eight bands each contain four fixed seeded hashes. Name keys are
combined with birth year; address keys with surname initial. These guards are
experimental choices, and their recall losses belong in the measurement. Empty
inputs have no keys. Buckets above 500 records are excluded in the experimental
snapshot. This is not a production incremental stoplist implementation.

```sh
uv run python benchmarks/full_pipeline.py --scale 1m --local --repeat 1 \
  --config configs/default.yaml --generator-profile hard-v1 \
  --blocking-experiments --lsh-seed 20260101 \
  --with-incremental --with-correction \
  --out artifacts/performance/keys-tuning
```

`run-001/blocking-experiments.json` compares current rules, dropping the broad rule,
and MinHash added to or replacing that rule, on the same trained model and TF
snapshot. Candidate counts are deduplicated across all keys. More keys in a union
cannot reduce candidates. More hashes/bits per band make a band stricter; more
bands increase retrieval. Preprocessing and match/cluster time are recorded, but
these exploratory timings are not complete-pipeline promotion evidence.

## Optional offline ONNX provider

The optional benchmark image uses BGE-small-en-v1.5 (384 dimensions, CLS pooling,
L2 normalization, 64-token truncation), eight bands of sixteen seeded random
projection bits, CPU ONNX Runtime 1.23.2 and tokenizers 0.22.1. The model uses the
[MIT license](https://huggingface.co/BAAI/bge-small-en-v1.5/tree/5c38ec7c405ec4b44b94cc5a9bb96e735b38267a).
The immutable revision and four SHA-256 checksums live in `vector_runtime.py`.
`vector-requirements.lock` pins and hashes the additional environment, constrained
against the root lockfile. Dependencies and weights download only during the build.

```sh
docker build -f benchmarks/Dockerfile.vectors \
  --build-arg BASE_IMAGE=er-pipeline:ci -t er-vector-bench:local .
docker run --rm --network none er-vector-bench:local \
  python benchmarks/vector_runtime.py --doctor /opt/er-vector-model
ER_VECTOR_IMAGE=$(docker image inspect er-vector-bench:local --format '{{.Id}}')
uv run python benchmarks/full_pipeline.py --image "$ER_VECTOR_IMAGE" \
  --scale 1m --local --repeat 1 --config configs/default.yaml \
  --generator-profile hard-v1 --blocking-experiments --lsh-seed 20260101 \
  --onnx-directory /opt/er-vector-model \
  --out artifacts/performance/onnx-tuning
```

Build from the same frozen application image used for controls and record its ID.
The runtime check validates every additional package pin and model checksum before
inference. No PyTorch, GPU, VSS extension or runtime model download is used. ONNX
memory is outside DuckDB's limit and is included in container measurements.
Regenerate the optional dependency lock deliberately using:

```sh
uv export --frozen --no-dev --no-emit-project --no-hashes \
  --output-file /tmp/er-vector-base-constraints.txt
uv pip compile benchmarks/vector-requirements.in \
  --constraint /tmp/er-vector-base-constraints.txt --python-version 3.12 \
  --universal --generate-hashes --output-file benchmarks/vector-requirements.lock
```

Production vectors, a persistent cache, frozen stoplist refresh and coherence remain
conditional on measurements. A dense runtime must first repay preprocessing cost;
production activation then needs a separate schema/config contract, fixture vectors,
`er doctor` checks, T-BLK-1 parity and incremental/full tests. Approximate HNSW is
only a possible diagnostic comparator; exact search on small samples and labeled
pairs are the recall references. EmbeddingGemma/Eridu, Rust ports, distributed
embedding and Kubernetes remain deferred. Coherence findings alone receive no
credit for improving cluster precision.
