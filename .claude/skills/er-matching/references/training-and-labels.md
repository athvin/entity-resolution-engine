# Training: how each parameter is estimated

## Contents

- What "training" covers, and what it never touches
- The prior
- u, by deterministic sampling
- m, by EM
- m, from labels
- The label queue entities
- Rules for changing a model

## What "training" covers, and what it never touches

A Fellegi–Sunter model needs three things: the prior (how likely any two random
records match), the **u** probabilities (level agreement given a non-match) and
the **m** probabilities (level agreement given a match). Each is estimated by a
different mechanism, in `rust/crates/dedupe-core/src/training/`.

Training never touches:

- **The comparison and blocking structure.** That is the settings template — a
  declaration, not a fitted value. Changing it is a config change.
- **Term frequency.** TF is a property of a corpus, frozen into a snapshot. It
  is registered, not trained. See `invariants.md` on INV-SCORE/D4.

Training is always explicit. The full-pipeline command uses an existing model
and never trains one — see the er-engine-ops skill.

## The prior

`training/prior.rs` — `estimate_probability_two_random_records_match`.

Deterministic rules produce pairs that are taken as matches; the count is
divided by the total possible comparisons and scaled up by the recall the
caller claims for those rules:

```
prob = (num_observed / recall) / num_total_comparisons
```

The deterministic pass uses the **same** "AND NOT preceding" dedup as blocking,
so a pair is counted once no matter how many rules would have caught it.

`recall` is the judgement call in this estimate: it says what fraction of true
matches the deterministic rules are expected to find. Getting it wrong scales
the prior directly.

## u, by deterministic sampling

`training/estimate_u.rs`.

Random pairs are overwhelmingly non-matches, so u can be estimated by sampling
rather than inference:

1. Deterministic hash sampling of records (deterministic so the estimate is
   reproducible — not a seeded RNG that drifts with row order).
2. A per-comparison "block-and-count" over the cartesian product of the sample,
   with RHS-only hash chunking and a probe pass.
3. Early stopping once every level has at least `min_count_per_level`
   observations.
4. `u_level = count_level / Σ count over non-null levels`.

Two details that matter when reading results:

- A level with zero observations becomes the **not-observed sentinel**, not
  zero. A literal zero would make its match weight infinite.
- **TF adjustments are disabled during u estimation.** u is a base rate; TF
  adjustment is applied at scoring time on top of it, and applying it twice
  would double-count.

## m, by EM

`training/em.rs`.

EM alternates between assigning match probabilities to candidate pairs and
re-estimating m from those soft assignments, over pairs produced by an
**EM-session blocking rule** supplied for the session.

The non-obvious rule: EM deliberately **does not train comparisons consumed by
its own training blocking rule**. If the session blocks on exact surname
equality, every pair in the session agrees on surname, so the surname
comparison carries no information in that session — estimating it there would
bake the blocking rule into the model. Train such a comparison in a different
session with a different blocking rule.

This is why training takes a *list* of EM blocking rules: between them they
must leave every comparison trainable in at least one session.

## m, from labels

`training/m_from_labels.rs` — `estimate_m_from_label_column` and
`estimate_m_from_pairwise_labels`.

Where ground truth exists, m can be counted directly instead of inferred.
Known-match pairs are generated (by equality of a label column, or read from a
labels table), their comparison vectors computed, and every pair treated as a
match:

```
m_i = count(level i among matches) / Σ counts over non-null levels
```

Properties worth knowing:

- **Only m is trained.** u and the prior are untouched by this path.
- Values **combine across training runs by median**, so several partial label
  sets can contribute without one dominating.
- Null levels are excluded from the denominator, same as everywhere else.

## The label queue entities

Labels are first-class data in the control plane, not a side file. The relevant
entities are `pair_label`, `matching_model` and `scored_record` — see the
entity-framework skill for their declarations and the generated accessors.

This is what makes a human-in-the-loop training path possible: a steward's
decision on a pair becomes a `pair_label` row, which feeds the from-labels
estimator above. It is a different mechanism from a steward **assertion** —
an assertion forces or forbids a merge for that specific pair; a label is
training evidence about the model. Do not conflate them: an assertion changes
one outcome, a label changes every future score.

## Rules for changing a model

1. A parameter change is a **new `model_version`**. Editing m/u under an
   existing version violates INV-SCORE, because a score must be reproducible
   from that version forever.
2. A new TF snapshot is a new `tf_snapshot_id`, for the same reason.
3. Model artifacts are plain files — settings/parameters JSON and the TF
   snapshot CSV. Ship them like any other artifact.
4. Scenario tests **never train**: EM over `base_10`'s 23 records is
   degenerate, so they pin a committed model.
5. After a model change, regenerate the affected goldens and say so explicitly
   in the commit message — a silent golden refresh is indistinguishable from a
   regression that was papered over.
