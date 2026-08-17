# Fixture & expected-output format

DesignDoc.md S8.2.1 is the authority. This file is its operational restatement for
whoever is authoring a scenario, and `scripts/validate_fixtures.py` is its enforcement.

Everything quoted here is asserted equal to the constants in `tests/helpers/scenario.py`
by `tests/unit/fixtures/test_fixture_format.py`, so this document cannot drift from the
code that reads the files.

## Directory shape

Every scenario is one directory with the same shape. `expected/` lives **inside** each
scenario; there is no top-level `fixtures/expected/` (S3).

```text
fixtures/static/<scenario>/
├── scenario.yaml             # the manifest: which phases this scenario has
├── base/                     # first delivery: crm.csv, billing.csv, webforms.csv
├── batch/                    # incremental delivery (omit when the scenario has none)
├── refresh/                  # --full-refresh-keys delivery
├── resurrect/                # ordinary delivery re-appearing a tombstoned key
├── assertions.csv            # input assertions, applied before the phase in their `phase` column
├── parity_pairs.csv          # optional: the pairs T-INC-3 scores through both code paths
├── tf_flip_pairs.csv         # optional: the edges T-INC-1b / T-TF-1 allow to cross auto_merge
├── truth.csv                 # ground truth: the persona label of each input row
├── traps.csv                 # ground truth: the designed traps and the rows that construct them
├── attribution.csv           # ground truth: each post-base record's role and discovering pass
└── expected/
    ├── base/                 # expected state after the base phase
    │   ├── membership.csv
    │   ├── golden.csv
    │   ├── events.csv
    │   ├── std_hashes.csv
    │   └── assertions.csv    # only where the scenario asserts on assertion state
    ├── batch/                # same five files, expected state after the batch phase
    ├── refresh/
    └── resurrect/
```

A phase directory that does not exist means the scenario has no such phase. An
`expected/<phase>/` file that does not exist means that phase makes **no claim** about that
relation — it is not an empty expectation, and `load_scenario` reports it as `None`.

## Phases

The phase vocabulary is exactly `{base, batch, refresh, resurrect}`. A phase is one delivery
plus the pipeline run over it. `base` is always present and always first; phases run in that
order over the phases the scenario has.

`resurrect` is not a spelling of `refresh`. It exists because a scenario needs a *second*
post-base delivery that is not itself a full refresh: reusing `refresh` would tombstone every
key the resurrection delivery omits, which is the opposite of what T-DEL-1 asserts.

The `phase` column of `assertions.csv` and every `expected/<phase>/` directory name are drawn
from this vocabulary and from no other.

## The manifest (`scenario.yaml`)

```text
scenario: incremental_batch        # required; must equal the directory name
phases: [base, batch]              # required; a subset of the vocabulary, always including base
base_scenario: base_10             # optional; whose deliveries run before this scenario's own
aux_files: [parity_pairs.csv]      # optional; which scenario-root files this scenario carries
bridged_labels: [E5, E6]           # optional; the base_scenario labels this scenario's bridge merges
bridged_personas: [P6, P7]         # optional; the personas holding those two labels
```

The manifest exists so that "the phases this scenario has" is a declaration rather than an
inference from whichever directories happen to be on disk: a delivery lost to a bad merge would
otherwise read as a scenario that never had that phase.

`bridged_labels` and `bridged_personas` are declared together or not at all, and each names
exactly two distinct entries. They exist for the same reason: a **bridge** record — one that
matches records of two different existing entities and so forces a merge — is cross-persona by
construction whenever the base corpus labels one entity per persona, which `base_10` does. There
is then no *truthful* bridge to build, so the merge has to be declared as a designed mechanic or
whoever finds it next reads it as a quality defect. A comment would not do: the tests assert that
the two labels are distinct groups of the `base_scenario`'s `expected/base/membership.csv` and
that the two personas holding them differ.

The grammar is deliberately small, and `scripts/validate_fixtures.py` refuses anything outside
it rather than guessing. One `key: value` per line, no indentation; a value is either a bare
scalar drawn from `[A-Za-z0-9_.-]` or a flow sequence `[a, b]` of them; `#` starts a comment.
The linter is a repo script run by a bare `python3` and imports the standard library only, so
it parses this grammar itself rather than depending on PyYAML.

`base_scenario` composes **inputs** only. A scenario declaring `base_scenario: ok_minimal`
replays that scenario's deliveries for any phase it does not provide itself, but its
expectations, its `assertions.csv` and its aux files are its own — absence-is-no-claim is only
readable when absence is local, and an inherited claim would be one no file in the scenario
states. A cycle in the chain is an error naming both scenarios.

## The two kinds of scenario-root file

The root holds no expectations, which is why its files sit beside `base/` rather than under
`expected/`. They fall into two kinds, and the distinction is normative because the linter
enumerates the root by name and rejects anything unlisted.

**Inputs and bounds** — `assertions.csv`, `parity_pairs.csv`, `tf_flip_pairs.csv`. These are fed
to the pipeline or bound what it may do. Each has a literal header below, and each must be
declared in the manifest's `aux_files`.

**Ground truth** — `truth.csv`, `traps.csv` and `attribution.csv`. These are read only by tests
and by the S8.5 metrics; the pipeline never sees them and no phase consumes them. `truth.csv`
carries one row per input record giving its persona label, and is what makes pairwise
precision/recall computable at all. `traps.csv` indexes each designed trap of S8.2 to the
`(source_system, source_record_id)` rows that construct it, so a fixture edit that dissolves a
trap fails a test instead of silently weakening the corpus. `attribution.csv` belongs to
incremental scenarios only: its columns are `record_key,role,pass`, it gives each post-base
`record_key` one role (`joiner`, `bridge`, `new_pair`) and the pass that MUST discover it
(`pass1` for new-vs-corpus, `pass2` for new-vs-new, per S4.3.4/D2), and without it the
new-vs-new pass can be omitted entirely while every downstream assertion still passes. It is a
CSV and not a manifest block because the manifest is a flat `key: value` map whose scalar
alphabet excludes the `:` inside every `record_key`. All three exist only in hand-authored
scenarios — a generated corpus carries its labels in the generator manifest instead — and none is
declared in `aux_files` nor appears in the header block below: that block is the linter's
grading table and S8.2.1 pins a literal there for the inputs and bounds alone. Ground truth is
graded by the scenario's own test instead, which is where `attribution.csv`'s three columns are
asserted.

## Headers (literal)

```text
assertions.csv                     # scenario root, an INPUT
phase,rec_a_key,rec_b_key,kind,created_by,note

parity_pairs.csv                   # scenario root, an INPUT
rec_a_key,rec_b_key

tf_flip_pairs.csv                  # scenario root, a BOUND
rec_a_key,rec_b_key,direction

expected/<phase>/membership.csv
persona_id,source_system,source_record_id,entity_label

expected/<phase>/golden.csv
entity_label,given_name,family_name,email,phone_e164,addr_number,addr_street,addr_unit,addr_city,addr_region,addr_postal,birth_date,survivorship_version

expected/<phase>/events.csv
entity_label,event_type,count

expected/<phase>/std_hashes.csv
source_system,source_record_id,std_hash

expected/<phase>/assertions.csv
rec_a_key,rec_b_key,kind,active
```

`direction` in `tf_flip_pairs.csv` is one of `{up, down}`: the side of `auto_merge` the edge may
move to.

`entity_label` is a **symbolic** name drawn from `E1, E2, … En`, allocated in ascending order of
the minimum `record_key` in the expected group. It MUST NEVER be a ULID: a pasted `entity_id`
passes on the run it was captured from and fails on every other one, so the linter refuses one.

`golden.csv` carries every `golden_records` column except `entity_id` (replaced by
`entity_label`) and `assembled_at` (a `VOLATILE_COLUMNS` member). `std_hash` is the SHA-256
defined by T-STD-1 over the stable column list of `int_std_records`.

## Encoding rules

Normative for both authoring and comparison:

- **Null token:** the two-character sequence `\N`. An empty field is the empty string, which is
  a distinct value. Nulls are never written as an empty field, and never as `NULL`, `None` or
  `NaN` — the linter rejects those spellings.
- **Float tolerance:** `1e-9`, absolute. Applied to `match_probability` and to any numeric golden
  column. All other columns compare exactly, after both sides are read as `VARCHAR`.
- **Sort key:** every expected file is stored sorted ascending, byte-wise on the UTF-8 encoding
  of the full column tuple in header order. The comparison helpers re-sort both sides before
  comparing, so a mis-sorted committed file is a lint failure here and never a scenario-test
  failure.
- **Excluded columns:** the `VOLATILE_COLUMNS` set of S5.0 — imported from
  `src/er/lake/columns.py`, never re-listed — never appears in an expected file.

## What the linter checks

`python3 scripts/validate_fixtures.py` lints every scenario under `fixtures/static/`;
`python3 scripts/validate_fixtures.py PATH ...` lints the scenarios you name. Each violation is
reported as `file:line: rule: message`, and `python3 scripts/validate_fixtures.py --list-rules`
prints the vocabulary.

| Rule | What it catches |
|---|---|
| `manifest` | a missing or malformed `scenario.yaml`, a phase outside the vocabulary, `batch` without `base`, an `aux_files` entry that is not there, a bridge declared with one of its two keys or naming other than two distinct entries |
| `phase-dir` | a directory that is not a phase, a phase directory the manifest does not declare, a declared phase with no delivery |
| `expected-phase` | an `expected/<phase>/` for a phase the scenario does not have |
| `unknown-file` | a scenario-root or `expected/` file the format does not define, or an input/bound the manifest does not declare |
| `header` | a header row that is not the S8.2.1 literal, or a row whose field count differs from it |
| `volatile-column` | a `VOLATILE_COLUMNS` member committed as a column |
| `entity-label` | an `entity_label` that is empty, NULL or a ULID |
| `null-token` | NULL written as something other than `\N` |
| `sort-order` | an expected file whose rows are not in ascending byte order |
| `assertion-phase` | an `assertions.csv` `phase` value outside the vocabulary, or naming a phase the scenario does not have |

Every rule has a committed negative self-test fixture under `tests/fixtures/scenarios/`, and
`tests/unit/fixtures/test_fixture_format.py` parametrises over `--list-rules` — so a rule added
without a fixture that fails for exactly that reason fails the suite. A linter with no failing
arm proves nothing.

`tests/unit/test_fixture_lint.py` discovers scenarios by walking `fixtures/static/`, so a
scenario added by a later ticket comes under lint with no edit to the test or the linter.
