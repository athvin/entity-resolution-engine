# Cost: what tells you *that*, and what tells you *why*

## Contents

- Budgets answer "that", and only by email
- CUR 2.0 is the only thing that answers "why"
- Split cost allocation, and why per-tenant cost is tractable
- Querying it: there is no Athena yet
- The tag-activation trap, restated
- Knowing when a tenant's cost means something is wrong

## Budgets answer "that", and only by email

Two monthly COST budgets exist, at **$300** and **$600**, each with an `ACTUAL`
and a `FORECASTED` notification at 100% of its limit. Every notification goes to
**one email address** and nowhere else — there is no SNS topic in the account,
so there is no path to Slack, PagerDuty or a Lambda.

The two-budget ladder is the whole signal: crossing $300 is "look at this", and
crossing $600 is "something is wrong". Neither tells you what moved, and neither
can be queried.

## CUR 2.0 is the only thing that answers "why"

`infra/terraform/envs/core/cur.tf` declares an `aws_bcmdataexports_export` on
the `use1` provider alias (BCM Data Exports is a `us-east-1`-only API, which is
why that alias exists at all — the rest of the account is `us-east-2`).

The settings that matter when reading the data:

| Setting | Value | Consequence |
|---|---|---|
| `TIME_GRANULARITY` | `HOURLY` | an idle-suspend window or a single run is visible; daily granularity would hide both |
| `INCLUDE_RESOURCES` | `TRUE` | per-resource rows, so a specific bucket or cluster can be attributed |
| `INCLUDE_SPLIT_COST_ALLOCATION_DATA` | `TRUE` | per-**pod** rows — see below |
| format / compression | `PARQUET` | columnar, so a partitioned query reads only the columns it needs |
| `refresh_cadence` | `SYNCHRONOUS` | restatements overwrite rather than append |

Delivered to the `er-cur-*` bucket under the `cur2` prefix, with a bucket policy
granting `bcm-data-exports.amazonaws.com` and `billingreports.amazonaws.com`,
conditioned on SourceAccount and SourceArn.

`envs/core/cur-query.sql` is the generated column selection the export is
defined by — a wide `SELECT` over `COST_AND_USAGE_REPORT`. Read it to know which
columns exist before writing a query against the delivered Parquet.

## Split cost allocation, and why per-tenant cost is tractable

A Kubernetes pod is not an AWS resource, so ordinary resource tags cannot see
it. Split cost allocation adds per-pod rows that divide each instance's cost
across the pods that ran on it, with namespace and workload as columns.

Two attributions fall out of design choices made for other reasons:

- **Per-developer cost is free**, because a developer environment *is* a
  namespace.
- **Per-tenant compute cost is tractable**, because a run *is* a dedicated
  single-tenant pod.

That second one is the load-bearing consequence. It is also why the metric
guidance in `docs/infrastructure.md` §10 forbids a `tenant` label on CloudWatch
metric series: tenant count is unbounded by design and every distinct label
value is a separate time series, so the bill would scale with
tenants × metrics × resolution. **Per-tenant cost comes from the CUR and the
`runs` table; per-tenant *metrics* do not exist and should not be added.** Use
`resource_class` as the metric dimension.

## Querying it: there is no Athena yet

There is **no Athena workgroup, no Glue catalog and no `aws_ce_*` resource** in
the account. The design doc notes that anything programmatic — a per-tenant
monthly figure, a cost-per-million-records trend — needs the CUR queried with
Athena, and that it must be partitioned or every query scans the whole history.

So today the options are the Billing console's Cost Explorer views, or reading
the Parquet directly. Reading it directly is unusually cheap here, because the
engine already is a DuckDB process that reads Parquet from S3 — a one-off
analysis does not need new infrastructure.

Standing up Athena or Glue for this is a real addition, not a configuration
change, and it is the kind of thing the `aws-data-analytics` plugin was
deliberately left out to avoid being nudged toward. Decide it as infrastructure.

## The tag-activation trap, restated

Every resource carries `Project=er`, `Environment`, `ManagedBy` and, on
per-developer resources, `Developer`, through the provider's `default_tags`. The
tags are therefore always present on new resources.

Presence is not activation. A tag becomes a **cost allocation** dimension only
once activated in the Billing console, activation takes up to 24 hours, and it
**does not backfill**. Spend from before activation is permanently
unattributable — no re-run, re-export or support case recovers it.

The verification is correspondingly behavioural rather than declarative:
confirm the tags show as *Active* in Billing, and confirm a CUR file has landed
with the split-cost-allocation columns present. Neither can be checked from the
Terraform.

## Knowing when a tenant's cost means something is wrong

The useful framing is cost per unit of work, not cost per tenant. A tenant whose
spend is high because its corpus is large is working correctly; a tenant whose
**cost per million records** is out of line with the others is the signal.

Two cheap checks before reaching for the CUR:

- Compare against the committed performance measurements. vCPU is the time lever
  and memory is near-constant in corpus size, so a run that cost far more than
  its record count predicts usually spilled.
- Check whether the namespace was suspended. Idle cost here is a
  namespace-suspend problem, and an un-suspended developer environment is a more
  common cause of a bill moving than any tenant's workload.
