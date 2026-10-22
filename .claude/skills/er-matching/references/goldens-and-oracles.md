# Goldens and oracles

## Contents

- Why the engine is golden-governed, not analyser-governed
- The three golden datasets
- The MANIFEST gate
- The Splink parity oracle
- Why the comparison runs in SQL
- Regenerating, and how to report it
- Toolchain provenance and version drift

## Why the engine is golden-governed, not analyser-governed

Every persistent SQL statement in this repo is normally a declared `.sql` +
`.yaml` pair that `er-sql-check` plans statically. `dedupe-core`'s model SQL is
the deliberate exception: its shape depends on the trained model (how many
comparisons, how many levels, which TF adjustments), so there is no fixed
statement to declare.

It is governed instead by an **execution oracle** — captured fixtures of the
SQL the engine emits and the output it produces. Do not add analyser coverage
to `dedupe-core` model SQL; the entity-framework skill says the same thing from
the other side.

## The three golden datasets

`rust/fixtures/golden/`:

| Dataset | Covers |
|---|---|
| `fake_1000` | the main correctness corpus — predict, vectors, EM, u, clustering, accuracy, incremental |
| `exploding` | array-column blocking rules, where candidate counts can blow up |
| `historical_50k` | scale; gated behind an env var and run on main/nightly, not per-PR |

Each holds the emitted SQL per stage (`predict_01___dedupe__blocked_id_pairs.sql`,
`predict_05___dedupe__comparison_vectors.sql`, …) alongside the resulting
Parquet/JSON. So the exact SQL a release emits is readable without running
anything — which is the fastest way to answer "what changed?" after a refactor.

Scenario fixtures (`base_10`, the assertion/merge/split/supersession/deletion
cases, `model_test_v1.json` + its `.tf.csv`) live under `fixtures/static/` and
serve the ER stages rather than the engine.

## The MANIFEST gate

Each dataset carries a `MANIFEST.json` holding a SHA-256 for every fixture file
— on the order of 180 files for `fake_1000` — and a test validates the tree
against it.

Its purpose is specifically to catch **accidental edits and partial
regeneration**. Partial regeneration is the dangerous one: regenerate the
predict fixtures but not the clustering fixtures and every individual test
still passes, while the suite as a whole no longer describes one coherent run.
The manifest makes that state fail.

So: never hand-edit a golden file, and never regenerate a subset. If a
regeneration is needed, regenerate the dataset and commit the manifest with it.

## The Splink parity oracle

`rust/crates/er-match/tests/gen_match_golden.py` replicates the scoring core
with **direct Splink 5.0.0 calls** (`from splink import DuckDBAPI, Linker`),
using:

- the same committed settings model,
- frozen TF injected via `register_term_frequency_lookup` — never recomputed,
  which is D4/INV-SCORE on the oracle side too,
- prediction at the same review threshold,
- the same `least`/`greatest` pair canonicalization and highest-probability
  dedup,
- `SET threads = 1` and `SET preserve_insertion_order = true`, so the output is
  byte-stable.

The Rust path reproduces it to **1e-10**. Run it with
`uv run python rust/crates/er-match/tests/gen_match_golden.py`.

Sibling parity generators exist for the other stages — `er-reconcile`
(cluster, never-cut, contradiction, reconcile), `er-ingest`, `er-std`,
`er-config`. They follow the same pattern: a Python reference implementation
committed next to the baseline it produced.

**Splink's presence is provenance, not a leftover.** The oracle, the
`splink_udfs` extension, `MANIFEST.generated_with`, and the historical banners
in the specification are all load-bearing evidence that the Rust rewrite
matches the implementation it replaced. Removing them would make that claim
unfalsifiable.

## Why the comparison runs in SQL

The probability comparison is done **in SQL, DOUBLE against DOUBLE**, not by
formatting both sides to text and comparing strings.

A float printed to 15 significant digits and re-parsed does not necessarily
round-trip, and 1e-10 is a tight enough margin that a text round-trip can
consume it. Comparing in SQL keeps the values in their native representation,
so the tolerance means what it says.

If a parity test starts failing by an amount suspiciously close to the
tolerance, check whether a code path introduced a string round-trip before
concluding the maths changed.

## Regenerating, and how to report it

1. Regenerate the whole affected dataset, never a subset.
2. Commit the refreshed `MANIFEST.json` in the same commit.
3. **Say in the commit message that goldens moved and why.** A silent golden
   refresh is indistinguishable from a regression that was papered over, and it
   is the one change a reviewer cannot evaluate from the diff alone — the diff
   is thousands of lines of Parquet hashes.
4. If the change was supposed to be behaviour-preserving and goldens moved
   anyway, that is a finding, not an inconvenience.

## Toolchain provenance and version drift

`MANIFEST.generated_with` records the toolchain that produced the fixtures —
the Python, Splink, DuckDB and pandas versions. That field is the first thing
to read when a golden fails for no apparent reason.

Note the DuckDB version recorded there is the one the baselines were generated
on, and the workspace's `duckdb` pin has moved since (it is an `=` pin precisely
so this is a deliberate, reviewable event — see the rust-conventions skill's
crate map for the current value). A DuckDB upgrade can legitimately change
emitted SQL or floating-point output, so before treating a golden diff as a
code regression, check whether the pin moved.
