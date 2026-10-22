# Evals: er-matching

Hand-run scenarios. Pose each in a **fresh** session, then check the expected
behaviours. These exist to catch the specific failures this skill was written
to prevent, not to confirm Claude knows what Fellegi–Sunter is.

Record a result as pass only if every "must" holds.

## 1. The frozen-TF trap

**Query:** "I'm adding a new scoring path in `er-match` that scores an
incremental batch against the corpus. Which dedupe-core entry point should it
call?"

Must:

- Choose `run_predict_with_tf`, not `run_predict`.
- Give the reason: `run_predict` computes TF from the input it is handed, so
  scores would depend on batch composition.
- Cite **INV-SCORE** (or D4) and name the TF snapshot as the source of the TF
  tables.

Must not:

- Suggest recomputing term frequency from the batch "for accuracy".
- Treat the two functions as interchangeable or a performance choice.

## 2. Incremental diverged from full

**Query:** "Our incremental run produced different entity IDs than a full run
over the same data. Is INV-EQ broken?"

Must:

- Say that differing **entity ids are not a violation** — INV-EQ compares the
  set-partition of current members, and ids are explicitly out of scope.
- Ask for / enumerate the five INV-EQ preconditions before calling it a bug.
- Not propose a fix to the clustering code on this evidence.

Credit for also noting that if a precondition *was* violated, the divergence is
correct behaviour and `er correct` is the repair.

## 3. Tuning the compile scale

**Query:** "The compiled model isn't merging enough duplicates on this tenant.
Should I raise `DEFAULT_FULL_AGREEMENT_BITS` from 60 to 120?"

Must:

- Say no, and give the measured reason: at 120 bits `base_10` finds 9 clusters
  — it starts **merging distinct people**. 55–60 is the calibrated stable band.
- Redirect to the actual lever: which comparison's declared weight is wrong, or
  whether a blocking rule is failing to emit the pair at all.

Must not:

- Treat the constant as a tuning knob, or reason that more evidence per field
  is more conservative.

## 4. "Why didn't these two records match?"

**Query:** "Records A and B are obviously the same person but they didn't get
merged. Where do I look?"

Must:

- Work **down** the pipeline, starting at `__dedupe__blocked_id_pairs`: if no
  blocking rule emitted the pair, no score exists and comparison config is
  irrelevant.
- Then name `__dedupe__comparison_vectors` (gamma levels) and
  `__dedupe__match_weight_parts` (per-comparison contribution) in that order.

Credit for noting that an unexpected gamma level usually means the
*standardized* value is not what you expect, which is an `er-standardize`
question rather than a matching one.

## 5. Changing a comparison level (routing + invariant)

**Query:** "I want to loosen the given_name Jaro–Winkler threshold from 0.90 to
0.85."

Must:

- Say the parameter change requires a **new `model_version`**; editing under
  the existing version violates INV-SCORE.
- Mention regenerating the affected goldens and stating it in the commit.

Must not:

- Route this to the entity-framework skill (no table or statement is changing),
  or propose editing a golden fixture by hand.
