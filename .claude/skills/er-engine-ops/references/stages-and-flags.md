# Stages, flags, and the drift guard

## Contents

- The command surface (generated)
- The logical stamps and their fixed defaults
- Flags that must travel together
- `run-all`: mode, escalation, segments
- The drift guard, and why `--allow-escalate` exists
- Training knobs that are judgements, not tuning
- Development lake recipes

## The command surface

Every subcommand, every flag, every default — derived from the clap definition
rather than copied from it, because this table is where a hand-maintained copy
went stale four separate ways.

<!-- BEGIN GENERATED: er-command-surface -->
Derived from `rust/crates/er-cli/src/cli.rs`: the clap `Cli` globals and
every `Command` variant, each flag carrying the default the code
declares. **required** means clap has no default and refuses the
invocation without it; `off` is a boolean switch; `optional` is an
`Option<…>` the stage treats as absent.

Each blurb is the subcommand's clap `about`, so this block answers
*what*. Everything about *why* — which flags must travel together, which
default is load-bearing — is prose outside these markers.

### The globals, on every subcommand

| Flag | Default |
|---|---|
| `--catalog` | **required** |
| `--data-path` | **required** |
| `--config` | **required** |

### `er ingest`

S4.1 — land one source's delivery into raw_records.

| Flag | Default |
|---|---|
| `--source` | **required** |
| `--path` | **required** |
| `--full-refresh` | off |
| `--ingest-batch-id` | `er-batch` |
| `--ingested-at` | `2026-01-01 00:00:00` |

### `er standardize`

S4.2 — build int_std_records. Needs the nickname lexicon seed.

| Flag | Default |
|---|---|
| `--lexicon` | **required** |
| `--ignore-words` | optional |
| `--run-at` | `2026-01-01 00:00:00` |

### `er match`

S4.3 — score match_scores against the committed model (full mode).

| Flag | Default |
|---|---|
| `--model` | **required** |
| `--tf` | **required** |
| `--model-version` | **required** |
| `--tf-snapshot-id` | **required** |
| `--run-id` | `er-run` |
| `--scored-at` | `2026-01-01 00:00:00` |

### `er reconcile`

S4.5 — cluster + reconcile into entity_membership/entities/entity_events.

| Flag | Default |
|---|---|
| `--run-id` | `er-run` |
| `--run-at` | `2026-01-01 00:00:00` |

### `er golden`

S4.6 — assemble golden_records + golden_lineage.

| Flag | Default |
|---|---|
| `--run-at` | `2026-01-01 00:00:00` |

### `er run-all`

The S4.0 run chain: ingest every `<drop>/<source>.csv` → standardize → match → reconcile → golden, under one run id.

| Flag | Default |
|---|---|
| `--drop` | **required** |
| `--lexicon` | **required** |
| `--ignore-words` | optional |
| `--model` | **required** |
| `--tf` | **required** |
| `--model-version` | **required** |
| `--tf-snapshot-id` | **required** |
| `--run-id` | `er-run` |
| `--mode` | `full` |
| `--allow-escalate` | off |
| `--segment` | optional |

### `er train`

S4.3.2 — train a match model on int_std_records; writes the trained settings JSON + the frozen TF snapshot CSV.

| Flag | Default |
|---|---|
| `--template` | **required** |
| `--deterministic-rule` | **required**, repeatable |
| `--em-rule` | **required**, repeatable |
| `--recall` | `0.9` |
| `--max-pairs` | `1e7` |
| `--model-version` | **required** |
| `--tf-snapshot-id` | **required** |
| `--out-model` | **required** |
| `--out-tf` | **required** |

### `er correct`

Apply staged corrections: re-reconcile (assertion-adjusted) + golden over the existing match_scores, without re-ingesting or re-scoring.

| Flag | Default |
|---|---|
| `--run-id` | `er-run` |
| `--run-at` | `2026-01-01 00:00:00` |

### `er doctor`

Read-only health report over the lake + config (JSON on stdout).

No flags.

### `er maintain`

Lake maintenance: flush inlined rows, merge small files, expire old snapshots. Prints a before/after file-count report (JSON).

| Flag | Default |
|---|---|
| `--expire-older-than` | `7 days` |

### `er profile`

Profile a corpus stage: per-column shape, most frequent values and coded findings, persisted to the lake and printed as JSON. Re-profiling an unchanged lake snapshot rewrites its rows rather than appending, so the stored result is a cache: asking twice costs one run.

| Flag | Default |
|---|---|
| `--subject` | `standardized` |
| `--source` | optional |
| `--tier` | `1` |
| `--exact-distinct` | off |
| `--sample` | optional |
| `--sample-seed` | optional |
| `--run-at` | `2026-01-01 00:00:00` |
<!-- END GENERATED: er-command-surface -->

## The logical stamps and their fixed defaults

`--ingested-at`, `--run-at` and `--scored-at` all default to a fixed literal
timestamp (above), **not** to the current time.

That is deliberate and worth preserving. These stamps are *inputs* to the
result: given the same data and the same stamps, a stage produces byte-identical
relations, which is exactly what the e2e goldens assert. Defaulting them to
`now()` would make every run unique and every golden unmaintainable.

So when scripting real operations, pass real stamps explicitly. When writing a
test, leave the defaults alone.

## Flags that must travel together

**`match`'s four model arguments are a set.** `--model`, `--tf`,
`--model-version` and `--tf-snapshot-id` are all required together: you cannot
score without declaring which model and which TF snapshot produced the scores,
because that pair is part of a score's identity.

**`ingest --full-refresh` changes what absence means.** Without it, ingestion is
append-only: it detects changed content per record key and appends a new
version. With it, a record the file omits is treated as deleted. Note that a
full refresh breaks INV-EQ's append-only precondition, so it is not a drop-in
substitute.

**`maintain` expires by default.** `--expire-older-than` is not an opt-in — it
has a default, so a bare `er maintain` expires snapshots at that window. Pass a
window longer than your longest-lived cursor rather than relying on the flag's
absence. Ordering of the maintenance steps is the ducklake-lakehouse skill's
subject; what this one owns is that the expiry happens whether or not you asked.

**`profile --subject raw` needs `--source`.** Two sources' raw payloads share no
schema, so the raw subject cannot be profiled across sources. The standardized
and golden subjects need no source.

## `run-all`: mode, escalation, segments

Takes the union of the stage inputs plus three decisions:

**`--mode`**

- `full` re-scores the corpus.
- `incremental` scores only the pairs touching new, changed and departed
  records, and reuses the rest. It is *equal to* a full pass precisely because
  the model and the TF snapshot are frozen — which is the operational face of
  INV-EQ.

**`--segment <name>`** restricts the match pass to a `segments` entry from the
config: that slice is scored against the **whole corpus**, and the rest is
reused. It bounds how much work one run does *without* losing cross-segment
duplicates — which a naive "only compare within the segment" split would.

**`--allow-escalate`** — see below.

## The drift guard, and why `--allow-escalate` exists

`er-versions` computes a run fingerprint over the config, model,
standardization, survivorship and lexicon. If that fingerprint has moved since
the last successful run, an incremental run is **refused** by default.

The reasoning is INV-EQ's precondition set. An incremental pass is only
equivalent to a full pass while the model, TF snapshot, `std_version` and
`config_hash` are unchanged. Once any of them moves, reusing previously
computed scores is no longer sound — the reused scores were produced under
different rules. So the default behaviour is to stop rather than quietly
produce a universe that is neither one thing nor the other.

`--allow-escalate` changes the response from *refuse* to **promote the run to
full**. That is usually what an operator wants: the work still happens, it just
costs a full pass. Use it in scheduled operation. Leave it off when you want to
be told that something moved.

The guard only reads `incremental`. A `full` run never consults the fingerprint,
so "the config changed" is never why a full run refuses to start.

This is a good mechanism to understand before changing config: a config edit
makes the next incremental run either fail or silently become expensive,
depending on this flag.

## Training knobs that are judgements, not tuning

`--recall` scales the prior directly (see the er-matching skill's training
reference) — it is a judgement about how complete the deterministic rules are,
not a knob to adjust until results look right.

Each `--em-rule` is a separate EM session, and a session cannot train a
comparison its own blocking rule consumed. Between them the rules must leave
every comparison trainable somewhere.

## Development lake recipes

A lake is a directory. From the repository root, where the fixtures and configs
the flags below name are rooted:

```sh
export ER_LAKE=artifacts/demo && mkdir -p "$ER_LAKE"
```

The committed `base_10` fixture is a complete three-source delivery (CRM,
billing, web forms) with a frozen, committed model — the same inputs the e2e
suites pin byte-exact expectations against. Use it rather than inventing a
corpus; a scenario that reproduces against `base_10` is one others can check.

Teardown is `rm -rf "$ER_LAKE"`. There is nothing else to clean up, which is
also why a corrupted dev lake is never worth debugging — delete and re-run.

The full cloud stack (API, dispatcher, UI, a per-developer namespace) is a
separate path; see the infra-platform skill.
