# Tenant config → model: `er-compile`

## Contents

- The gap this closes
- A classic model is a Fellegi–Sunter model
- The comparator token set
- Weights
- The 60-bit calibration, and why louder is worse
- Null policy and derived keys
- Provenance: compiled is not trained

## The gap this closes

A tenant config has always declared its matching intent:

```yaml
comparisons:
  given_name: { levels: [exact, "jaro_winkler:0.90", variant_match], tf: true }
  email:      { levels: [exact, username_exact], tf: true }
blocking:
  - { key_type: email_exact, expr: email }
```

That declaration was validated at publish, canonicalized into `config_hash`,
and rendered in the config studio — and then **never read by anything that
ran**. The level tokens were free strings with no interpreter. The SQL the
engine actually executed came from a model JSON named by `ER_MODEL_PATH`,
authored by hand, out of band. So every "configurable matching" capability was
configurable only by editing a file on a disk the tenant cannot reach.

`er-compile` is that missing interpreter.

It is deliberately **not a new language**. The token set is read off the
committed `model_test_v1.json`, so compiling the committed `test.yaml`
reproduces that model's structure level-for-level — asserted by
`er-compile`'s own `tests/compile.rs`. What the compiler adds on top is the
parts of the DSL that previously had nowhere to live: per-field weights, an
explicit null policy, and derived keys.

## A classic model is a Fellegi–Sunter model

This is the design decision worth understanding before touching the crate.

A competitor ships two engines here — a "classic" weighted-sum matcher and a
trained probabilistic one. This repo ships one. A classic model is a
Fellegi–Sunter model whose parameters were **declared rather than learned**:
`weights_to_probabilities` converts each field's declared weight into the m/u
pair that makes that field contribute the intended share of the score.

The payoff is that a hand-weighted model still produces the same evidence
waterfall, the same per-pair explanation and the same threshold semantics as a
trained one. There is no second scoring path to keep honest, and no class of
bug that exists only in "classic" mode.

So: do not add a parallel weighted-sum code path. If a capability seems to need
one, it needs a weight→probability mapping instead.

## The comparator token set

The compiler rejects anything it does not know, by name, with the full list in
the error — the message is shown to whoever wrote the config, so it has to be
actionable:

| Token | Parameter |
|---|---|
| `exact` | — |
| `jaro_winkler:<0..1>` | threshold |
| `levenshtein:<n>` | max edit distance |
| `variant_match` | — (uses the lexicon) |
| `username_exact` | — (email local part) |
| `domain_exact` | — (email domain) |
| `first_n:<n>` | prefix length |
| `dob_same_year_month` | — |
| `date_within_days:<n>` | tolerance in days |
| `date_transposed` | — |
| `numeric_within:<tol>` | absolute tolerance |
| `numeric_within_pct:<pct>` | relative tolerance |
| `address_components` | — (component group) |

Adding a comparator means adding a token here **and** the SQL that implements
its level, and then deciding whether the analyser can plan that SQL — see the
sql-analysis skill on function stubs.

## Weights

Either every comparison leaves its weight unset, in which case they are shared
equally, or the declared weights **total exactly 100**. A partial set is an
error, not a silent normalization, because a config that totals 80 is far more
likely to be a mistake than an intent. Each weight is also range-checked to
`0..=100`.

The failure messages name the total and the offending column, so a publish
rejection is self-explaining without reading this file.

## The 60-bit calibration, and why louder is worse

Full agreement across a model's comparisons is scaled to **60 bits** of log2
evidence (`DEFAULT_FULL_AGREEMENT_BITS`). This is calibrated, not a default
someone picked for roundness, and the intuition most people bring to it is
backwards.

Measured on `base_10`:

| Scale | Clusters found | Reading |
|---|---|---|
| 20 bits | 18 | too quiet — fails to merge records that are the same person |
| 55–60 bits | matches the committed membership exactly | the stable region |
| 120 bits | 9 | too loud — **merging distinct people** |

More evidence per field does not mean more caution. Past the calibrated region
the model becomes confident about pairs that merely agree on a few fields, and
false merges are the expensive error: a wrong merge destroys two entities'
identity and needs a steward unmerge, whereas a missed merge leaves a duplicate
that the next pass can still catch.

55–60 reproduces the committed membership across every disagreement ratio
tried, so 60 sits inside the stable band rather than at its edge. Treat it as a
calibrated constant. If a change appears to need a different scale, the real
question is which comparison's weight is wrong.

## Null policy and derived keys

Two capabilities the DSL declared but could not express before:

- **Null policy** — what a missing value means for a comparison. Making it
  explicit matters because the alternative is an implicit convention, and the
  two reasonable conventions (a null is "no evidence" versus a null is
  "disagreement") give materially different scores.
- **Derived keys** — a blocking key computed from columns rather than being a
  column, which is what lets a tenant block on something the schema does not
  literally store.

## Provenance: compiled is not trained

`compile` records in the model's own provenance that its parameters were
declared, not estimated. A compiled model is a **starting point**: when
training runs it replaces the declared parameters with estimated ones under a
new `model_version`, per the rules in `training-and-labels.md`.

That provenance field is the thing to check when a model's numbers look
suspiciously round — they probably are, because nobody has trained it yet.
