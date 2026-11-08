# Tenant config → model: `er-compile`

## Contents

- The gap this closes
- A classic model is a Fellegi–Sunter model
- The comparator token set
- Weights
- Cross-field comparison groups
- The 60-bit calibration, and why louder is worse
- Null policy and derived keys
- Hard constraints and the hierarchy exclusion
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
| `contains` | — (substring, either direction) |
| `datetime_within_hours:<n>` | tolerance in hours (TIMESTAMP columns) |
| `geo_within_km:<km>` | radius over a "lat,lng" text column |

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

## Cross-field comparison groups

`comparison_groups:` pools attributes so any member on one record compares
against every member on the other:

```yaml
comparison_groups:
  any_phone:
    attributes: [mobile, home_phone]
    levels: [exact]
```

The case it exists for: a CRM holds one phone in `mobile` and the same phone
in `home_phone` constantly, and same-column comparison scores that pair as
disagreement on two fields — worse than no evidence, because it pushes a true
duplicate below threshold.

A group compiles to **one pooled comparison** plus **one appended blocking
rule**, so a cross-field agreement can generate the candidate as well as score
it. Each declared level is OR'd over the full attribute cross-product, the
diagonal included — two records that both carry the value in `mobile` must
agree too. The engine side needed nothing new: `dedupe-core` parses a
comparison's input columns from its level SQL, so the member columns ride into
`blocked_with_cols` on their own.

The refusals are the design, not omissions:

- **`tf: true` is refused** — a pool has no single term-frequency
  distribution, and attaching one member's `tf_` column would mis-weight the
  others.
- **`variant_match` and `address_components` are refused** — both are bound to
  fixed columns (the lexicon's `name_variants`, the schema's `addr_*`) and
  have no pairwise reading.
- **One attribute is refused** — that is a comparison, and declaring it as a
  group would only obscure it.
- **A group named like a comparison is refused** — both compile to the same
  `gamma_<name>` column.

Group weights draw from the same 100 as the per-column comparisons: the
totals-100 rule applies to the union. Null policy reads over the pool — a side
whose members are ALL blank is the "no evidence" case — with the same three
policies as a per-column comparison.

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
tried, so 60 sits inside the stable band rather than at its edge.

### The calibration is person-only, and that is a recorded gap

Both legs of that derivation — summing the committed model's own exact-level
Bayes factors, and the grid search above — were run on the committed **person**
model. `compile` hands every config the same constant: `compile_with` never
consults `config.entity_type`, and the production entry point passes
`CompileOptions::default()`.

The committed **account** model measures far less full-agreement evidence over
its four comparisons, so an account config is compiled substantially louder
than its own reference model — in the direction this section calls the
expensive mistake. `er-compile/tests/score_scale_calibration.rs` holds the two
measurements and the reason it is deliberately a measurement rather than a fix:
the person number earned its value from two independent legs, and only the
Bayes-factor leg can be run for accounts today, because the committed account
corpus has no declared truth or trap set to grid-search against. Read that file
before quoting a number.

So: on a person config, treat 60 as a calibrated constant, and if a change
appears to need a different scale the real question is which comparison's
weight is wrong. On an account config, the scale itself is the open question,
and `compile` now reads `thresholds.full_agreement_bits` so a corpus with truth
can be pointed at.

**The knob only exists on the compile path.** `full_agreement_bits` scales
parameters as a model is compiled, so it does nothing to a model that was
loaded from a committed JSON — which is what scenario tests pin. A grid search
that produces a flat cluster count is almost always that: the knob never
reached the model under test.

## Null policy and derived keys

Two capabilities the DSL declared but could not express before:

- **Null policy** — what a missing value means for a comparison. Making it
  explicit matters because the alternative is an implicit convention, and the
  two reasonable conventions (a null is "no evidence" versus a null is
  "disagreement") give materially different scores.
- **Derived keys** — a blocking key computed from columns rather than being a
  column, which is what lets a tenant block on something the schema does not
  literally store.

## Hard constraints and the hierarchy exclusion

`constraints` compiles to conjuncts on EVERY blocking rule, so a violating
pair is never generated as a candidate — "before similarity counts" is
literally true there. Three column forms (`required_match`, `veto`,
`exclude_when_equal`) and one structural form:

```yaml
constraints:
  exclude_hierarchy:
    parent: ParentId            # a metadata key: the raw source column holding
                                # the parent record's source id
    # include_parent_child: true  # the opt-out toggle
```

`exclude_when_equal` on a parent column sees ONE level; `exclude_hierarchy`
walks the chain. The compiler freezes a `NOT EXISTS` conjunct against
`__dedupe__hierarchy_ancestors` — a depth-capped recursive closure the engine
materializes from the concat before blocking runs (`settings.hierarchy` is
what tells it to; the conjunct and that block always travel together) — so a
grandparent and a grandchild never pair while siblings, who share a parent but
sit on no chain together, still do. **The default is exclusion: declaring the
hierarchy turns it on**, and `include_parent_child: true` is the recorded
opt-out (the spec's "Include Parent/Child" toggle). The parent reference is a
`metadata` key because raw linkage columns are deliberately not canonical
attributes; a key absent from the data builds no chain and excludes nothing.

## Provenance: compiled is not trained

`compile` records in the model's own provenance that its parameters were
declared, not estimated. A compiled model is a **starting point**: when
training runs it replaces the declared parameters with estimated ones under a
new `model_version`, per the rules in `training-and-labels.md`.

That provenance field is the thing to check when a model's numbers look
suspiciously round — they probably are, because nobody has trained it yet.
