# Operating the engine

*Engine layer — operating one tenant's lake via the `er` CLI. The multi-tenant
control plane that runs these same operations as jobs is
[backend-design.md](../../er-server-control-plane/references/backend-design.md); the
[README](../../../../README.md#architecture-at-a-glance) has the whole platform.*

Commands below run from the repository root — but **cargo does not**: the
workspace root is `rust/` and there is no manifest at the repository root, so
the build is the one line here that needs a subshell. Training is always
explicit: `er run-all` uses an existing model, it never trains one. Every
command takes the same three globals — a DuckLake catalog path, a data
directory, and the S6 config:

```sh
(cd rust && cargo build -p er-cli --bin er)
er() { rust/target/debug/er \
  --catalog "$ER_LAKE/meta.ducklake" --data-path "$ER_LAKE/data" \
  --config configs/test.yaml "$@"; }
```

## Contents

- [Development lake](#development-lake)
- [First load](#first-load)
- [Training](#training)
- [Incremental deliveries and corrections](#incremental-deliveries-and-corrections)
- [Maintenance](#maintenance)
- [Inspecting the lake](#inspecting-the-lake)
- [The dev cloud stack](#the-dev-cloud-stack)
- [Querying the cost report](#querying-the-cost-report)

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

`er doctor` is the read-only health report: tenant, config hash, the std and
survivorship versions, the latest run's id/status/mode, a presence/row-count
line per relation, and an `ok` flag true only when `int_std_records`,
`entity_membership` and `golden_records` all exist:

```sh
er doctor
```

`ok: false` exits `1`, so read the JSON rather than branching on the status —
an unhealthy lake is a finding, not a failed command.

`er profile` is the other read: per-column shape, frequent values and coded
findings for the `raw` (needs `--source`), `standardized` or `golden` stage,
persisted to the lake. Re-profiling an unchanged snapshot rewrites its rows
instead of appending, so asking twice costs one run.

```sh
er profile --subject standardized --exact-distinct
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
er maintain                                 # flush, merge — AND expire, at the
                                            # flag's default window
er maintain --expire-older-than "30 days"   # the window you actually chose
```

Prints a before/after file-count report. **`--expire-older-than` is not an
opt-in:** it has a default and `run_maintain` calls `expire_snapshots`
unconditionally, so there is no invocation that only compacts. Snapshot expiry
bounds the lake's history window — the read API pins snapshots, so keep the
window longer than your longest-lived cursor, and pass it explicitly in
anything scheduled rather than relying on the flag's absence.

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
[infrastructure.md](../../infra-platform/references/infrastructure.md).

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

**Or skip Athena entirely for one-off work.** The export is Parquet and the
whole of October is under a megabyte, so `duckdb` answers the same questions
locally for nothing — no scan charge, no result bucket, and the partition key
comes through as a column:

```sh
aws s3 cp s3://er-cur-797781631727/cur2/er-cur2/data/ /tmp/cur/ \
  --recursive --exclude '*' --include '*.parquet'
duckdb -c "SELECT BILLING_PERIOD, round(sum(line_item_unblended_cost),2) AS usd
           FROM read_parquet('/tmp/cur/**/*.parquet', hive_partitioning=true)
           GROUP BY 1 ORDER BY 1;"
```

Every probe answer below was obtained this way. Athena earns its keep once the
history is large enough that downloading it stops being trivial, or when a
query needs to run from a job rather than a laptop.

### The three probes — and their answers as of 2026-11-02

The design in [infrastructure.md](../../infra-platform/references/infrastructure.md) §16.2 assumes things about
the CUR's shape that only a real billing month can confirm. These were run
against the delivered October and November partitions; **one of the three came
back badly and is why the per-pod cost model cannot be built yet.**

> **P1 — split cost allocation produces NOTHING.** All eleven
> `split_line_item_*` columns are present and **every value is NULL across
> 56,816 October rows**. The columns exist because `cur.tf` sets
> `INCLUDE_SPLIT_COST_ALLOCATION_DATA = "TRUE"` on the export; the data is
> absent because **split cost allocation has a second switch** — an
> account-level opt-in under *Billing → Cost Management Preferences* — which
> has never been enabled. The export flag includes the columns; the preference
> generates the data.
>
> It is free, it takes up to 24 hours, and like cost allocation tags it is
> **computed going forward and does not backfill**. So §16.2's "per-tenant
> compute cost is tractable because §6 makes a run a dedicated single-tenant
> pod" rests on data that is not being produced, and every day unenabled is a
> day of pod-level cost that cannot be recovered. **Enable it before building
> any per-run cost model.**

> **P2 — the Auto Mode fee is separately itemized, and therefore not split.**
> It bills as its own usage type, `USE2-EKS-Auto:<instance>-management-hours`
> ($9.15 across three instance types in October, ~12% of instance cost as
> D-AUTOMODE states). Because it is a distinct line rather than part of the
> instance's cost, it will **not** appear inside a per-pod split row once
> splitting is on — so a per-pod figure understates by roughly that 12% unless
> the fee is apportioned by `split_line_item_split_usage_ratio` separately.

> **P3 — the user tags are fine; the EKS ones are not.** `Project`,
> `Environment` and `ManagedBy` were activated 2026-10-11 and appear on all
> 42,502 tagged rows as `user_project` / `user_environment` /
> `user_managed_by`, so the §16.1 non-backfill warning was heeded for those.
> Two gaps remain: **`Developer` is on no resource at all** (`default_tags` in
> `envs/{core,dev}/versions.tf` never set it), so §16.1's "per-developer cost
> comes free" is false; and **`aws:eks:cluster-name` is `Inactive`** while the
> other five `aws:eks:*` keys the design expects do not exist as activatable
> tags at all — they only materialise once split cost allocation is on.

Two further observations worth recording, because the design's numbers assume
otherwise:

- **October was $184.23**, against the ~$190/mo dev estimate in §16 — the
  estimate was good. Top lines: EKS control plane $55.17 (554 h, a partial
  month), EC2 instances ~$47, NAT $31.87, RDS $19.87, EBS gp3 $11.39.
- **No runner-pool usage has happened yet.** §6.5 names `c6id` for the runner
  pools; October billed **0.3 hours** of `c6id.xlarge` in total. What actually
  ran is `c6g.large` (Graviton, $0.068/h), `m6a.large` and `m6i.large` — all
  **2-vCPU** instances, not the 8-vCPU `*.2xlarge` the sizing assumes. So the
  blended per-class rate the cost model needs **cannot be calibrated from this
  data**; it needs a month with real runs on it.

Re-run them with the queries below after changing anything.

### The re-run of 2026-11-05 — and the thing it found

Re-run three days later against the November partition (19,770 rows). One probe
is unchanged, and the spend picture underneath it is not.

> **P1 is still NULL.** `split_line_item_split_cost`,
> `split_line_item_split_usage_ratio` and `split_line_item_parent_resource_id`
> are **0 non-null out of 19,770 November rows**. The account-level preference
> has not been enabled, so three more days of pod-level cost are permanently
> unrecoverable. Nothing about the per-pod model has moved.

**What the re-run was not looking for: November is on pace for ~6x October.**

| | October (full month) | November (5 days) |
|---|---|---|
| total | **$184.23** | **$160.47** |

The top line is new. S3 was not in October's top five; in November it is
**$76.27**, and all of it is one thing:

| usage type | bucket | USD | units |
|---|---|---|---|
| `USE2-DataTransfer-Out-Bytes` | `er-ci-sccache` | **75.37** | 837 GB |
| `USE2-Requests-Tier2` | `er-ci-sccache` | 0.68 | 1,709,346 |

837 GB at $0.090/GB. That is **internet egress**: GitHub-hosted runners are not
in the VPC, so an S3 Gateway Endpoint cannot help and every sccache read leaves
the region at the public rate.

**Nov 4 isolates the cause, for free.** That is the day the Actions billing
block stopped CI from running at all:

| day | total USD | sccache USD |
|---|---|---|
| 2026-11-01 | 19.39 | 0.03 |
| 2026-11-02 | 32.48 | 12.46 |
| 2026-11-03 | 51.83 | 31.07 |
| **2026-11-04** | **21.55** | **0.00** |
| 2026-11-05 | 35.21 | 32.67 |

A day with no CI costs ~$20 and egresses nothing. A day with CI costs $32–52,
and the entire difference is this one line. Nov 1 reads as $0.03 only because
AWS's first 100 GB/month of egress is free — the cumulative allowance was
consumed during Nov 2, which is why the charge appears to start then rather than
scaling from the first run.

**This is the price of the CI speedup, and it was not previously measured.**
`rust-cache` was removed in #424, which made sccache the only cache; the test
job was then sharded, which multiplied the number of runners each pulling from
it; and the hit rate is ~99%, so almost every compilation unit downloads its
cached object. PR wall time went 32–35 min to ~10 min. At ~$31 on each day CI
runs, that is roughly **$600–900/month** for ~25 minutes of wall clock, against
a **$600 monthly budget** that will breach around mid-month.

The baseline rose too, independently: EC2 went from ~$1.50/day in October to
~$7.60/day. That is the observability stack, the second web-api Deployment from
the colour dimension (#461), and the dispatcher — all real, all new, none of it
sccache.

**This is a decision, not a bug.** The options, cheapest-to-change first:

1. **Accept it** and raise the budget. Fast CI is worth real money; what was
   wrong was not knowing the number.
2. **Move sccache to the GitHub Actions cache backend**
   (`SCCACHE_GHA_ENABLED`). Egress becomes free, but that cache is ~10 GB per
   repository with LRU eviction, which this workspace will exceed — so expect
   the hit rate, and the 10-minute wall time, to degrade. Measure before
   keeping.
3. **Self-hosted runners inside the VPC.** The structural fix for the egress
   itself: S3 traffic becomes in-region and free. **The second half of this
   argument has expired** — it used to read "*and* Actions minutes stop being
   metered on a private repository", which was true while the repository was
   private. Standard GitHub-hosted minutes are free for a public repository, so
   that saving is already banked and only the egress is left to win. Confirm
   which regime applies before costing this: `gh repo view --json visibility`.
   Note it also moves fork-PR code execution onto infrastructure inside the AWS
   account, which is a materially different exposure on a public repository than
   on a private one. The cluster, the runner NodePools and the pod plumbing
   already exist for the batch-orchestration work. Largest change, best
   end state.
4. **Narrow what sccache stores** so shards pull less. Smallest saving.

Do not resolve this by deleting the bucket's contents: a cold sccache costs a
42-minute PR, which is the problem it was built to fix.

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
