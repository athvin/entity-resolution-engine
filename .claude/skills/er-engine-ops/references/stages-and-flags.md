# Stages, flags, and the drift guard

## Contents

- The globals
- The logical stamps and their fixed defaults
- Per-stage flags
- `run-all`: mode, escalation, segments
- The drift guard, and why `--allow-escalate` exists
- Training knobs
- Development lake recipes

## The globals

Every subcommand takes all three:

| Flag | Meaning |
|---|---|
| `--catalog` | the DuckLake catalog — a `.ducklake` or `sqlite:` path |
| `--data-path` | the lake's Parquet data directory |
| `--config` | the S6 config YAML |

## The logical stamps and their fixed defaults

`--ingested-at`, `--run-at` and `--scored-at` all default to the literal
`2026-01-01 00:00:00`, **not** to the current time.

That is deliberate and worth preserving. These stamps are *inputs* to the
result: given the same data and the same stamps, a stage produces byte-identical
relations, which is exactly what the e2e goldens assert. Defaulting them to
`now()` would make every run unique and every golden unmaintainable.

So when scripting real operations, pass real stamps explicitly. When writing a
test, leave the defaults alone.

## Per-stage flags

**`ingest`** — one source's delivery.

| Flag | Notes |
|---|---|
| `--source` | the source system name, as configured |
| `--path` | the delivery CSV |
| `--full-refresh` | tombstones records this source omitted |
| `--ingest-batch-id` | default `er-batch` |
| `--ingested-at` | logical stamp |

Without `--full-refresh`, ingestion is append-only: it detects changed content
per record key and appends a new version. With it, absence is meaningful — a
record the file omits is treated as deleted. Note that a full refresh breaks
INV-EQ's append-only precondition, so it is not a drop-in substitute.

**`standardize`** — `--lexicon` (the nickname/variant seed, required),
`--run-at`.

**`match`** — `--model`, `--tf`, `--model-version`, `--tf-snapshot-id`,
`--run-id` (default `er-run`), `--scored-at`. All four model/TF arguments are
required together: you cannot score without declaring which model and which TF
snapshot produced the scores, because that pair is part of a score's identity.

**`reconcile`** — `--run-id`, `--run-at`.

**`golden`** — `--run-at`.

**`correct`** — `--run-id`. Re-reconciles and re-assembles over existing
`match_scores`.

**`doctor`** — no flags. Read-only JSON report.

**`maintain`** — `--expire-older-than` plus the catalog-option flags described
in the ducklake-lakehouse skill.

## `run-all`: mode, escalation, segments

Takes the union of the stage inputs (`--drop`, `--lexicon`, `--model`, `--tf`,
`--model-version`, `--tf-snapshot-id`, `--run-id`) plus three decisions:

**`--mode`** (default `full`)

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

This is a good mechanism to understand before changing config: a config edit
makes the next incremental run either fail or silently become expensive,
depending on this flag.

## Training knobs

| Flag | Default | Notes |
|---|---|---|
| `--template` | — | the base settings: comparison + blocking structure |
| `--deterministic-rule` | — | repeatable, SQL; feeds the prior |
| `--em-rule` | — | repeatable, SQL; the EM session blocking rules |
| `--recall` | `0.9` | the recall credited to the deterministic rules |
| `--max-pairs` | `1e7` | caps the EM sample |
| `--model-version`, `--tf-snapshot-id` | — | the new identity being minted |
| `--out-model`, `--out-tf` | — | where the two artifacts are written |

`--recall` scales the prior directly (see the er-matching skill's training
reference) — it is a judgement about how complete the deterministic rules are,
not a tuning knob to adjust until results look right.

Each `--em-rule` is a separate EM session, and a session cannot train a
comparison its own blocking rule consumed. Between them the rules must leave
every comparison trainable somewhere.

## Development lake recipes

A lake is a directory:

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
