---
name: er-engine-ops
description: Explains how to drive this repo's entity-resolution pipeline with the `er` CLI — the stage chain (ingest, standardize, match, reconcile, golden), `run-all`, explicit training, the correction pass and what it does not repair, lake maintenance that always expires snapshots, the doctor and profile reports — plus the determinism contract reruns depend on, model and TF-snapshot identity, what the `er` binary's exit status omits, and who guarantees one writer per tenant. Use when running or scripting a pipeline stage, setting up a development lake, profiling a corpus, interpreting a run's exit code or failure, or deciding between a correction pass and a full rerun. For the matching maths and model internals see er-matching; for ATTACH options, lake layout and the maintenance step order see ducklake-lakehouse; for why a stage is slow see ducklake-performance; for the dispatcher loop, the job queue, the constraint that enforces one writer and the exit contract a task pod honours, see er-server-control-plane.
---

# Operating the engine

The `er` binary runs the pipeline over one tenant's lake. Every invocation takes
the same three globals — the DuckLake catalog, the Parquet data directory, and
the S6 config.

Two directories are in play and mixing them is the first thing that fails:
**cargo must run from `rust/`** (the workspace root — there is no manifest at
the repository root), while the fixtures, configs and lake paths every flag
names are rooted at the **repository root**.

```sh
(cd rust && cargo build -p er-cli --bin er)   # from the repo root
er() { rust/target/debug/er \
  --catalog "$ER_LAKE/meta.ducklake" --data-path "$ER_LAKE/data" \
  --config configs/test.yaml "$@"; }
```

A development lake is just a directory. Nothing to provision, nothing to tear
down beyond `rm -rf`:

```sh
export ER_LAKE=artifacts/demo && mkdir -p "$ER_LAKE"
```

The stage chain is `ingest` → `standardize` → `match` → `reconcile` → `golden`,
which `run-all` drives under one run id. Alongside it sit `train`, `correct`,
`maintain`, `doctor` and `profile`. Every subcommand, every flag and every
default is **derived from the clap definition** in
[references/stages-and-flags.md](references/stages-and-flags.md) — read it there
rather than from memory, because the hand-copied version of that table drifted
four ways at once.

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

## `correct` re-clusters; it does not re-score

`correct` applies staged steward corrections — assertions, overrides, overlays,
review resolutions — by re-reconciling (assertion-adjusted) and re-assembling
**over the existing `match_scores`**. No re-ingest, no re-score, and no new TF
snapshot: it reuses the `model_version` and `tf_snapshot_id` of the latest run,
because those are the identity the corpus was scored under. The thresholds it
clusters at travel on that run's row rather than being re-read from today's
config, so a config edit cannot retroactively re-cut yesterday's scores.

Reach for it when stewards have made decisions that need to take effect. That
is the whole of what it currently repairs.

**What it does not repair, despite what the design doc specifies.** S4.5.6 names
two INV-EQ loss vectors — incremental candidate generation, which can never pair
two records that were both already in the corpus, and corpus-dependent term
frequency — and specifies a correction pass that regenerates candidates over the
whole corpus, re-materializes TF under a new `tf_snapshot_id` and re-scores at
the active model version. **Neither arm is built.** The implemented pass is
reconcile → golden → reviews over scores that already exist; its own e2e test is
named for the absence (`correct_applies_a_staged_assertion_without_rescoring`).
So incremental drift is **not** repaired by `correct` today — a full rerun is
what restores INV-EQ, and T-INC-1b is the acceptance test still outstanding for
the TF arm.

Reach for a full rerun whenever the model, the config, or standardization
changed — those move scores, and `correct` deliberately does not re-score.

Incremental deliveries are just another `run-all` with a new drop directory and
run id; ingestion detects changed content per record key and appends versions.
A full-refresh delivery tombstones the records a source omitted
(`ingest --full-refresh`).

## Maintenance expires snapshots whether or not you ask

```sh
er maintain                                 # flush, merge — AND expire, at the
                                            # flag's default window
er maintain --expire-older-than "30 days"   # the window you actually chose
```

`--expire-older-than` has a default, and `run_maintain` calls
`expire_snapshots` unconditionally — so there is no invocation of `er maintain`
that only compacts. **Keep the window longer than your longest-lived cursor**:
every lake read pins a snapshot, so expiring history out from under a paginated
or resumed read breaks it, and the default is short enough to do that. Pass the
window explicitly in anything scheduled. Ordering of the maintenance steps
matters; see
[ducklake-lakehouse](../ducklake-lakehouse/references/layout-and-maintenance.md).

## What an `er` exit status tells you

Narrower than it looks, and the distinction is the whole point:

> **The `er` binary exits `0` or `1`.** It does not depend on `er-errors` at
> all. Any failure arrives as an `anyhow` error out of `main`, which is exit
> `1`; `doctor` additionally exits `1` when its report is not `ok`, which is a
> *finding*, not a crash.

The exit-code contract — including the code that surprises people, **exit `10`
is success** ("nothing to do") — belongs to `er-server exec-task`, the task-pod
verb that runs a stage under the dispatcher, and to `run_stages.error_class`
that goes with it. That is where a wrapper has to treat `0` and `10` alike, and
where `2` (config) and `3` (precondition) are distinguishable. The classes,
their exit codes and how to read a failure:
[references/exit-codes-and-failures.md](references/exit-codes-and-failures.md).

Reading this backwards costs real time in both directions: scripting `er` with
a branch for exit `2` writes a branch that can never be taken, and reading a
pod's exit `10` as a failure reports healthy no-op runs as broken.

## One writer per tenant — whose job that is

A tenant's lake must have one writer at a time, and on `main` that guarantee
comes from the **control plane's queue**, not from the CLI: the jobs table
admits at most one active job per org — `org` is the control plane's name for
the same subject the engine calls a tenant — enforced by Postgres. When the
DuckLake catalog is itself Postgres, the catalog also serializes writers.

The gap worth knowing: **running `er` directly bypasses the queue.** Two manual
runs against one lake are not protected by anything the control plane does. For
local work that is fine because you are the only writer; do not build an
operational process on it. The queue's own constraints are the
er-server-control-plane skill's subject.

## Inspecting a run

`er doctor` is the read-only health report — the tenant, the config hash and
the std/survivorship versions, the latest run's id/status/mode, a
presence/row-count line per relation, and an `ok` flag that is true only when
`int_std_records`, `entity_membership` and `golden_records` all exist. It is the
right first command when a lake is in an unknown state, because it reads
nothing it could damage. Note the exit status above: `ok: false` is exit `1`, so
a script must read the JSON rather than branch on the status.

`er profile` is the other read: per-column shape, frequent values and coded
findings for the raw, standardized or golden stage, persisted to the lake.
Re-profiling an unchanged snapshot rewrites its rows instead of appending, so
asking twice costs one run.

For browsing actual data, the read API and the UI are the supported surfaces.
A `duckdb` shell works for ad-hoc inspection but bypasses snapshot pinning and
role checks.
