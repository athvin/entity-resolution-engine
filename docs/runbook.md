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

## Querying the cost report

`envs/core/athena.tf` puts a Glue table (`er_cur.cur2`) and a workgroup
(`er-cost`) over the CUR 2.0 export `cur.tf` has been delivering since Phase 0.
Query it with `AWS_PROFILE=er-mgmt`, always filtering on `billing_period` — the
workgroup caps a query at 10 GiB scanned, so an unfiltered query fails rather
than bills.

```sh
aws athena start-query-execution \
  --work-group er-cost \
  --query-execution-context Database=er_cur \
  --query-string "$(cat query.sql)"
```

### The three probes — run these before writing any production query

The design in [infrastructure.md](infrastructure.md) §16.2 assumes things about
the CUR's shape that only a real billing month can confirm. Run these first and
let the answers shape the queries, rather than assuming and finding out later.

**P1 — what do split rows actually key on?** The design joins a pod to its run
through `task_instances.pod_uid`. This shows what identity the split rows
actually carry and in what format.

```sql
SELECT line_item_resource_id, split_line_item_parent_resource_id, resource_tags,
       line_item_usage_type, split_line_item_split_usage_ratio,
       split_line_item_split_cost, split_line_item_unused_cost
FROM er_cur.cur2
WHERE billing_period = '2026-10' AND split_line_item_split_cost IS NOT NULL
LIMIT 25;
```

**P2 — is the EKS Auto Mode fee split per pod?** It is ~12% of on-demand per
instance-hour and is charged *even on Spot* (D-AUTOMODE). If it is not split,
per-pod billed cost understates by roughly that much, and the allocated model
has to apportion it by `split_line_item_split_usage_ratio`.

```sql
SELECT line_item_usage_type, count(*) AS rows, sum(line_item_unblended_cost) AS cost,
       sum(CASE WHEN split_line_item_split_cost IS NULL THEN 0 ELSE 1 END) AS split_rows
FROM er_cur.cur2
WHERE billing_period = '2026-10'
  AND line_item_product_code IN ('AmazonEKS', 'AmazonEC2')
GROUP BY 1 ORDER BY cost DESC;
```

**P3 — which cost allocation tags are actually populated? Run this one first.**
§16.1: activation "takes up to 24 hours and *does not backfill*. Activate them
late and the first weeks of spend are permanently unattributable." The CUR has
been delivering since Phase 0, so an empty result for the `aws:eks:*` keys means
spend is being lost to attribution right now, and every day of delay loses more.

```sql
SELECT k, count(*) AS rows
FROM er_cur.cur2
CROSS JOIN UNNEST(map_keys(resource_tags)) AS t(k)
WHERE billing_period = '2026-10'
GROUP BY 1 ORDER BY 2 DESC;
```

Two known gaps to check against the result: `default_tags` in
`envs/{core,dev}/versions.tf` sets only `Project` / `Environment` / `ManagedBy`,
so the `Developer` tag §16.1 names for per-developer attribution is **not** on
any resource; and `Project` is `entity-resolution`, not the `er` that §16.1's
prose claims. Activation itself is a Billing console action — check whether
`aws_ce_cost_allocation_tag` can manage the AWS-generated `aws:eks:*` keys
before putting it on a path that applies unattended on `main`.
