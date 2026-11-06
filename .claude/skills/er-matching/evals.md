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
- Treat the entry points as interchangeable or a performance choice.
- Offer `run_predict_chunked` as the scalable alternative: it recomputes TF
  from the input exactly as `run_predict` does.

Credit for stating the rule as "frozen TF from the snapshot, whichever
composition" rather than as "call this one function" — the committed match
stage composes concat / block / `predict_pipeline` itself and is not a
violation.

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
- Ask which entity type the tenant is, or state the scoping unprompted: the
  band was calibrated on the committed person model, and the committed account
  model measures much less full-agreement evidence, so 60 is already loud for
  an account config.

Must not:

- Treat the constant as a tuning knob, or reason that more evidence per field
  is more conservative.
- Present 60 as calibrated for every entity type.

Credit for naming `er-compile/tests/score_scale_calibration.rs` as the place
the measurements live, and for knowing the knob applies only when a model is
**compiled** — a committed model JSON is unaffected by it, so a grid search
over a pinned model would show a flat cluster count.

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

## 6. The Splink cleanup

**Query:** "There's still Splink stuff all over this repo — a Python oracle, a
`splink_udfs` extension, Splink banners in the design doc — and the engine is
Rust now. Can you clean that up?"

Must:

- Decline, and say what the remnants are **for**: they are the evidence that
  the Rust path reproduces the implementation it replaced, so removing them
  makes the parity claim unfalsifiable rather than tidy.
- Name the `__dedupe__*` rename as the part that was already done, so the
  prefix is not confused with the provenance.

Must not:

- Offer a staged deletion, or treat the oracle as a dev-only script.

## 7. Training a comparison the session blocks on

**Query:** "EM gave me a useless m for `surname`. The session blocks on exact
surname equality — should I raise the iteration count?"

Must:

- Say the iteration count is not the problem: every pair in that session agrees
  on surname by construction, so the comparison carries no information there
  and EM deliberately does not train a comparison consumed by its own session
  blocking rule.
- Say the fix is another EM session with a different blocking rule, and that
  the rule list must between them leave every comparison trainable somewhere.

Must not:

- Suggest removing the exclusion, or treat the result as a convergence bug.

## 8. The match stage is slow (selection boundary)

**Query:** "The match stage takes 40 minutes on our biggest tenant. What do I
change?"

Must:

- Keep the matching-side levers here: which blocking rule is emitting the
  candidate pairs, whether an exploding array rule is multiplying them, and
  that the cut between blocking and scoring is where a saving comes from.
- Hand the measurement discipline and any storage-layout proposal to the
  `ducklake-performance` skill rather than asserting a cause.

Must not:

- Reach for `run_predict_chunked` as the fix — it bounds memory and recomputes
  TF, so it is not available on the ER path.
- Propose a partition or sort key on the strength of a wall-clock number alone.

## 9. The grid search that shows nothing

**Query:** "I swept `full_agreement_bits` from 20 to 200 against
`model_test_v1.json` and the cluster count never moved. Is the calibration
claim wrong?"

Must:

- Say the sweep never reached the model: the constant scales parameters during
  `compile`, and a committed model JSON arrives with its values already fitted.
- Redirect to compiling a config at each scale instead.

Must not:

- Conclude the committed measurements are wrong, or propose editing them.
