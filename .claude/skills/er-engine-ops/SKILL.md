---
name: er-engine-ops
description: Explains how to drive this repo's entity-resolution pipeline with the `er` CLI — the stage chain (ingest, standardize, match, reconcile, golden), `run-all`, explicit training, the correction pass, lake maintenance and the read-only doctor report — plus the determinism contract that reruns depend on, model and TF-snapshot identity, the exit-code contract, and who guarantees one writer per tenant. Use when running or scripting a pipeline stage, setting up a development lake, interpreting a run's exit code or failure, deciding between a correction pass and a full rerun, or wiring a stage into the dispatcher. For the matching maths and model internals see er-matching; for ATTACH options and lake layout see ducklake-lakehouse.
---

# Operating the engine

The `er` binary runs the pipeline over one tenant's lake. Every invocation takes
the same three globals — the DuckLake catalog, the Parquet data directory, and
the S6 config:

```sh
cargo build -p er-cli --bin er
er() { rust/target/debug/er \
  --catalog "$ER_LAKE/meta.ducklake" --data-path "$ER_LAKE/data" \
  --config configs/test.yaml "$@"; }
```

A development lake is just a directory. Nothing to provision, nothing to tear
down beyond `rm -rf`:

```sh
export ER_LAKE=artifacts/demo && mkdir -p "$ER_LAKE"
```

## The subcommands

| Command | Stage | Notes |
|---|---|---|
| `ingest` | S4.1 | lands one source's delivery into `raw_records` |
| `standardize` | S4.2 | builds `int_std_records`; needs the nickname lexicon seed |
| `match` | S4.3 | scores `match_scores` against a **committed** model |
| `reconcile` | S4.5 | clusters into `entity_membership` / `entities` / `entity_events` |
| `golden` | S4.6 | assembles `golden_records` + `golden_lineage` |
| `run-all` | S4.0 | the whole chain under one run id |
| `train` | S4.3.2 | fits a model; writes settings JSON + frozen TF snapshot CSV |
| `correct` | — | re-reconcile + re-assemble over **existing** scores |
| `maintain` | — | flush inlined, merge small files, expire snapshots |
| `doctor` | — | read-only health report (JSON on stdout) |

## `run-all` never trains

This is the single most important thing about the CLI. `run-all` **uses** an
existing model; it never fits one. Training is always a separate, explicit
`train` invocation.

The reason is that training is not idempotent and not cheap, and a pipeline
that silently retrained would make every run's scores a function of whatever
corpus it happened to see — which INV-SCORE forbids. See the er-matching skill.

So a first load passes a model and TF snapshot in:

```sh
er run-all \
  --drop fixtures/static/base_10/base \
  --lexicon rust/crates/er-cli/tests/fixtures/nickname_variants.csv \
  --model fixtures/static/model_test_v1.json \
  --tf fixtures/static/model_test_v1.tf.csv \
  --model-version v0001 --tf-snapshot-id tf0001 \
  --run-id demo-run-1
```

`--drop` is a directory of `<source>.csv` files; each is ingested as that
source.

## Model artifacts are files, and identity is never reused

`train` writes two plain files — the trained settings JSON and the frozen
term-frequency snapshot CSV. Commit or ship them like any other artifact, then
pass them to `run-all` or `match`.

Scores are a function of `(model_version, tf_snapshot_id)`, so **a new model is
a new identity, never an in-place overwrite.** Reusing a version string for
different parameters makes every historical score unreproducible and silently
breaks the incremental equivalence argument.

Training needs a settings template (the comparison + blocking structure),
deterministic rules for the prior, and EM blocking rules:

```sh
er train --template <settings.json> \
  --deterministic-rule "l.email = r.email" \
  --em-rule "l.family_name = r.family_name" \
  --model-version v0002 --tf-snapshot-id tf0002 \
  --out-model <path> --out-tf <path>
```

## Determinism is a contract, not a nicety

Stage commands take the run-logical stamps explicitly — `--run-at`,
`--ingested-at`, `--scored-at`. Given the same inputs and the same stamps, a
rerun produces **byte-identical** relations. That is what the e2e goldens
assert, and it is why a resumed run reproduces rather than approximates.

Two consequences:

- Do not default a stamp to "now" in a code path a test will run. The stamps
  are parameters because wall-clock time is not an input to the result.
- A run is replayable from its recorded inputs. `runs` stores each run's
  configuration fingerprints, status and snapshot range for exactly that
  purpose.

## `correct` versus a full rerun

`correct` applies staged steward corrections — assertions, overrides, overlays,
review resolutions — by re-reconciling (assertion-adjusted) and re-assembling
**over the existing `match_scores`**. No re-ingest, no re-score.

Reach for it when:

- Stewards have made decisions that need to take effect.
- Incremental drift needs repairing. It refreshes term frequency under a new
  `tf_snapshot_id` and re-scores the regenerated candidate pairs at the
  **active** model version — which is precisely the repair for INV-EQ's two loss
  vectors. It never trains, so INV-PERM still governs the ids it rewrites.

Reach for a full rerun instead when the model, the config, or standardization
changed — those move scores, and `correct` deliberately does not re-score from
scratch.

Incremental deliveries are just another `run-all` with a new drop directory and
run id; ingestion detects changed content per record key and appends versions.
A full-refresh delivery tombstones the records a source omitted
(`ingest --full-refresh`).

## Maintenance and the snapshot window

```sh
er maintain                                 # flush inlined, merge small files
er maintain --expire-older-than "14 days"   # also expire old snapshots
```

Prints a before/after file-count report. **Keep the expiry window longer than
your longest-lived cursor** — every lake read pins a snapshot, so expiring
history out from under a paginated or resumed read breaks it. Ordering of the
maintenance steps matters; see
[ducklake-lakehouse](../ducklake-lakehouse/references/layout-and-maintenance.md).

## Exit codes

Uniform across every `er` command, from `er-errors`. The one that surprises
people:

> **Exit `10` is success** — "nothing to do". It sits deliberately outside the
> S4.7 failure table.

A script that treats any non-zero exit as failure will report healthy no-op
runs as broken. Check for `10` explicitly. The failure classes and their codes:
[references/exit-codes-and-failures.md](references/exit-codes-and-failures.md).

## One writer per tenant — whose job that is

A tenant's lake must have one writer at a time, and on `main` that guarantee
comes from the **control plane's queue**, not from the CLI: the jobs table
admits at most one active job per org, enforced by Postgres. When the DuckLake
catalog is itself Postgres, the catalog also serializes writers.

The gap worth knowing: **running `er` directly bypasses the queue.** Two manual
runs against one lake are not protected by anything the control plane does. For
local work that is fine because you are the only writer; do not build an
operational process on it.

## Inspecting a run

`er doctor` is the read-only health report — config hash, the latest run, and a
presence/row-count line per relation, as JSON. It is the right first command
when a lake is in an unknown state, because it reads nothing it could damage.

For browsing actual data, the read API and the UI are the supported surfaces.
A `duckdb` shell works for ad-hoc inspection but bypasses snapshot pinning and
role checks.

Stage-by-stage inputs, outputs and the flags each command takes:
[references/stages-and-flags.md](references/stages-and-flags.md).
