# Evals: er-engine-ops

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Exit 10 is not a failure

**Query:** "Our nightly wrapper reports the ER pipeline as failing, but the data
looks fine. The command exited 10."

Must:

- Identify `10` as **`NothingToDo` — a success code**, deliberately outside the
  S4.7 failure table.
- Say the wrapper should treat `0` and `10` as success.

Must not:

- Hunt for a bug in the pipeline, or map 10 onto an error class.

## 2. run-all does not train

**Query:** "Set up a fresh lake and get golden records out of this new CSV
drop. There's no model yet."

Must:

- Say `run-all` **uses** a model and never trains one, so a model must exist
  first — either the committed fixture model or an explicit `er train`.
- Pass `--model`, `--tf`, `--model-version` and `--tf-snapshot-id` to
  `run-all`.

Credit for using the committed `base_10` / `model_test_v1.json` fixtures rather
than training on a tiny corpus, and for noting EM over 23 records is
degenerate.

## 3. correct versus rerun

**Query:** "A steward resolved 40 review items and we also changed the
survivorship rules in the config. What do I run?"

Must:

- Separate the two causes: steward decisions are what `correct` is for, but a
  **config change moves results**, so this needs a full run, not `correct`.
- Note that `correct` deliberately does not re-score from scratch.

Credit for mentioning the drift guard: the changed `config_hash` will refuse an
incremental run unless `--allow-escalate` promotes it to full.

## 4. The stamps are inputs

**Query:** "The `--run-at` default is a hardcoded 2026-01-01 date. That looks
like a bug — should it default to now?"

Must:

- Say no: the logical stamps are **inputs to the result**, and fixed defaults
  are what make reruns byte-identical for the goldens.
- Say real operations should pass real stamps explicitly.

Must not:

- Agree it is a bug, or propose defaulting to the current time.

## 5. Concurrent writers

**Query:** "Two of us ran `er run-all` against the same dev lake at the same
time. Shouldn't the advisory lock have stopped that?"

Must:

- Say the one-writer-per-tenant guarantee comes from the **control plane's
  queue** (at most one active job per org, enforced by Postgres), and that
  running `er` directly **bypasses** it.
- Not assert that the CLI takes a per-tenant writer lock.

Credit for noting that a Postgres DuckLake catalog also serializes writers at
the catalog level, and that a corrupted dev lake is cheaper to delete than to
debug.
