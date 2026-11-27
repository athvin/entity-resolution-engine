# Customer-authored rules

What a customer can express today through the published config, how each rule
is validated, executed and versioned, and what is genuinely missing (#515).
The short version: **there is a rule language, and it is the config.** Each
block below is declarative, column-checked at validation, compiled into the
same SQL the batch pipeline and the online `/resolve` path both run, and
versioned with the config it belongs to. `pg_customer_rules` publishes all of
them together through a tenant admin key and pins the refusals.

## The rule blocks

| Block | Expresses | Checked before a version exists | Executes as |
|---|---|---|---|
| `transforms.<attribute>` | cleanse a raw value: `lower`, `upper`, `trim`, `replace`, `regex_replace`, `strip`, `substring`, `default`, `dictionary`, `named_dictionary`, `ignore_words` | canonical attribute; non-empty rule list; regex compiles; substring bounds; dictionary non-empty and named dictionaries exist | standardization SQL (`er-config` renders each op; `regex_replace` → DuckDB `regexp_replace`) |
| `replacement_dictionaries` | reusable directed mappings, by value or by word | shape (`dictionaries::validate`) | inlined into the transform SQL |
| `constraints` | `required_match`, `veto`, `exclude_when_equal`, `exclude_hierarchy` — hard candidate rules that similarity cannot outvote | each names a standardized column | conjuncts on every blocking rule |
| `blocking[]` | which pairs are candidates | SQL parses; every column exists | candidate generation |
| `comparisons`, `comparison_groups` | per-field and cross-field evidence levels, weights, TF | comparison keys are standardized columns; `tf` needs an `exact` level | the scoring stage |
| `segments` | a named slice a run may be restricted to | predicate parses; columns exist | a run filter (`POST …/jobs {segment}`) |
| `models` | an ensemble of registry models with thresholds | distinct non-empty names; each has an ACTIVE version | scoring + clustering |
| `survivorship`, `survivorship_groups` | which value wins, per field or for a field set from one record | group tokens, fields and chains coherent (a group replaces its members' own chains) | golden election (`er-golden`) |
| `thresholds`, `approval` | auto-merge / review band; automated-merge approval | finite numbers | clustering and review routing |

Every problem is reported at once by `POST …/config/validate` and refused by
publish with the same text, each naming what to change — for example
`survivorship_groups `full_name` has no survivorship chain; a group is decided
once, under its own name — add `survivorship.full_name``.

## The properties #515 asks for

- **Validated:** the checks above, in `config_check::config_reference_problems`
  plus `er_config::load_config_str`, before any draft is stored.
- **Versioned:** a config version is immutable; an edit is a new draft; runs
  record the `config_hash` they executed under; publish classifies the change
  into a rebuild tier (A/B/C) against the dataset's active version.
- **Bounded execution:** no block executes customer code. Expressions are SQL
  over standardized columns, parsed and column-checked; regex is DuckDB's
  `regexp_replace` (RE2: linear time, no backreferences), so a pathological
  pattern cannot backtrack. Execution time is bounded by the task timeout.
- **One evaluator for batch and online:** `/resolve` standardizes its payload
  "exactly as an ingested record would be" (`er_cli::resolve_record`) against
  the same published config.
- **Generic, not provider-specific:** nothing above names a provider. Salesforce
  master and writeback restrictions live in `writeback.rs` and the E9 children,
  at the adapter boundary.

## Missing, on evidence

- **Visual authoring.** Customers write YAML; there is no block designer.
  Deliberately not a prerequisite for the API (#515's own scope).
- **A rule language beyond these blocks** (arbitrary boolean rule trees,
  per-rule actions) has no customer requirement behind it yet. Add one from a
  real policy that the blocks cannot express, not from a parity table.
