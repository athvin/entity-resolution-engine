# Evals: er-engine-ops

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Exit 10 is success, and it is not the CLI's

**Query:** "Our nightly wrapper reports the ER pipeline as failing, but the data
looks fine. The job's pod exited 10."

Must:

- Identify `10` as **`NothingToDo` — a success code**, deliberately outside the
  S4.7 failure table, and say the wrapper should treat `0` and `10` as success.
- Place the contract where it lives: `er-server exec-task` and
  `run_stages.error_class`, **not** the `er` binary, which exits `0` or `1`.

Must not:

- Hunt for a bug in the pipeline, or map 10 onto an error class.
- Claim a local `er run-all` can exit 10, or suggest branching on exit 2 or 3
  around an `er` invocation.

Credit for noting that the kubelet calls any non-zero exit a `Failed` phase, so
exit 10's success is carried by the termination record.

## 2. run-all does not train

**Query:** "Set up a fresh lake and get golden records out of this new CSV
drop. There's no model yet."

Must:

- Load this skill without being told to, rather than answering from the `er`
  `--help` output or reaching for er-matching.
- Say `run-all` **uses** a model and never trains one, so a model must exist
  first — either the committed fixture model or an explicit `er train`.
- Pass `--model`, `--tf`, `--model-version` and `--tf-snapshot-id` to
  `run-all`.
- Build from `rust/` while keeping the fixture and config paths repo-root
  relative, or run the build through `make`.

Credit for using the committed `base_10` / `model_test_v1.json` fixtures rather
than training on a tiny corpus, and for noting EM over 23 records is
degenerate.

## 3. correct versus rerun

**Query:** "A steward resolved 40 review items and we also changed the
survivorship rules in the config. What do I run?"

Must:

- Separate the two causes: steward decisions are what `correct` is for, but a
  **config change moves results**, so this needs a full run, not `correct`.
- Note that `correct` re-reconciles over the existing `match_scores` and
  re-scores nothing.

Must not:

- Claim `correct` refreshes term frequency, mints a new `tf_snapshot_id`, or
  repairs incremental drift. The design doc specifies that pass; the code does
  not implement it.

Credit for mentioning the drift guard: the changed `config_hash` will refuse an
incremental run unless `--allow-escalate` promotes it to full — and for knowing
the guard reads `incremental` only, so a full run is never what it refuses.

## 4. The stamps are inputs

**Query:** "The `--run-at` default is a hardcoded date. That looks like a bug —
should it default to now?"

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

Credit for noting that `org` and `tenant` are the same subject at two layers,
that a Postgres DuckLake catalog also serializes writers at the catalog level,
and that a corrupted dev lake is cheaper to delete than to debug.

## 6. Maintenance is not opt-in

**Query:** "We have a paginated export that reads a lake snapshot for about an
hour. Is it safe to run `er maintain` with no flags while that's running?"

Must:

- Say **no**: `--expire-older-than` has a default and `maintain` expires
  unconditionally, so there is no compaction-only invocation.
- Say every lake read pins a snapshot, so expiring under a live cursor breaks
  the read, and tell the operator to pass a window longer than the longest-lived
  cursor.

Must not:

- Claim a bare `er maintain` only flushes and merges.
- Quote a window from memory instead of the generated command surface.
