# Operating the engine

*Engine layer — operating one tenant's lake via the `er` CLI. The multi-tenant
control plane that runs these same operations as jobs is
[backend-design.md](backend-design.md); the
[README](../README.md#architecture-at-a-glance) has the whole platform.*

Commands below run from the repository root. Training is always explicit:
`er run-all` uses an existing model, it never trains one. Every command takes the
same three globals — a DuckLake catalog path, a data directory, and the S6 config:

```sh
cargo build -p er-cli --bin er
er() { rust/target/debug/er \
  --catalog "$ER_LAKE/meta.ducklake" --data-path "$ER_LAKE/data" \
  --config configs/test.yaml "$@"; }
```

## Development lake

A development lake is just a directory — DuckLake keeps its metadata in the
`.ducklake` catalog file and its Parquet data under the data path. Nothing to
provision, nothing to tear down beyond `rm -rf`:

```sh
export ER_LAKE=artifacts/demo
mkdir -p "$ER_LAKE"
```

The committed `base_10` fixture is a complete three-source delivery (CRM,
billing, web forms) with a frozen, committed model — the same inputs the e2e
suites pin byte-exact expectations against.

## First load

One command runs the full chain — ingest every `<drop>/<source>.csv`, then
standardize → match → reconcile → golden — under one run id:

```sh
er run-all \
  --drop fixtures/static/base_10/base \
  --lexicon rust/crates/er-cli/tests/fixtures/nickname_variants.csv \
  --model fixtures/static/model_test_v1.json \
  --tf fixtures/static/model_test_v1.tf.csv \
  --model-version v0001 --tf-snapshot-id tf0001 \
  --run-id demo-run-1
```

`er doctor` is the read-only health report: config hash, the latest run, and a
presence/row-count line per relation:

```sh
er doctor
```

Each stage is also its own subcommand for stepwise operation —
`er ingest --source crm --path <csv>`, `er standardize --lexicon <csv>`,
`er match --model <json> --tf <csv> --model-version <v> --tf-snapshot-id <id>`,
`er reconcile`, `er golden`. Stage commands take the run-logical stamps
(`--run-at`, `--ingested-at`, `--scored-at`) explicitly; reruns with the same
inputs and stamps produce byte-identical relations — that determinism contract
is what the e2e goldens enforce.

## Training

Training fits the match model's parameters over the standardized corpus and
freezes the term-frequency snapshot beside it. It needs a settings template
(the comparison + blocking structure), deterministic rules for the prior, and
EM-session blocking rules:

```sh
er train \
  --template fixtures/static/model_test_v1.json \
  --deterministic-rule "l.email = r.email" \
  --em-rule "l.family_name = r.family_name" \
  --model-version v0002 --tf-snapshot-id tf0002 \
  --out-model artifacts/demo/trained.json \
  --out-tf artifacts/demo/trained.tf.csv
```

The outputs are plain files: commit or ship them like any artifact, then pass
them to `run-all`/`match`. Scores are a function of `(model_version,
tf_snapshot_id)` — a new model is a new identity, never an in-place overwrite.

## Incremental deliveries and corrections

A later delivery is another `run-all` with a new drop directory and run id;
ingestion detects changed content per record key and appends versions, and the
downstream stages rebuild from the current versions.

Steward corrections (assertions, overrides, overlays, review resolutions) are
staged by the control plane and applied by:

```sh
er correct --run-id demo-correct-1
```

which re-reconciles and re-assembles over the *existing* scores — no re-ingest,
no re-score. Full-refresh deliveries tombstone the records a source omitted
(`er ingest --full-refresh`).

## Maintenance

```sh
er maintain                      # flush inlined rows, merge small files,
er maintain --expire-older-than "14 days"   # expire old snapshots
```

Prints a before/after file-count report. Snapshot expiry bounds the lake's
history window — the read API pins snapshots, so keep the window longer than
your longest-lived cursor.

## Inspecting the lake

The product surfaces (read API + UI) are the supported way to browse golden
records, lineage, reviews and runs. For ad-hoc inspection, a `duckdb` shell
(1.1+, with the `ducklake` extension) attaches the same catalog read-only —
this is the exact ATTACH the engine itself issues:

```sh
duckdb -c "INSTALL ducklake; LOAD ducklake; \
  ATTACH 'ducklake:artifacts/demo/meta.ducklake' AS lake (DATA_PATH 'artifacts/demo/data', READ_ONLY); \
  SELECT id, status, mode FROM lake.main.runs;"
```

## The dev cloud stack

The engine-in-a-directory flow above is for local work. The full stack — API,
dispatcher, UI, per-developer namespace on the dev EKS cluster with Aurora
control plane and S3 lakes — is `make dev-up DEV=<name> TAG=<git sha>` (and
`dev-suspend` / `dev-resume` / `dev-down`); see
[infrastructure.md](infrastructure.md).
