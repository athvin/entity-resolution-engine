"""The `base_10` corpus and the properties it was authored to have (S8.2, S8.2.1).

Every M2-M4 scenario test reads this fixture, so a silent edit to it does not fail
here -- it fails four milestones downstream, in an integration test that points at
the pipeline rather than at the 23 rows that actually moved. These eight tests are
what turn each designed property back into a fixture failure.

Three of them are worth stating as design decisions rather than as assertions:

* **Nothing here is hard-coded that the corpus is graded against.** The headers are
  derived from `sources.<name>.columns` in `configs/test.yaml` AND read out of
  S8.2's literal block, and the corpus has to equal both -- so a header edit needs
  three agreeing edits, which is what makes V11 real instead of decorative. The
  trap vocabulary is likewise read out of S8.2's own table, so a trap added to the
  spec fails `traps.csv` rather than passing unnoticed.
* **The normalizers are the pipeline's, not this file's.** `email_norm` and
  `phone_e164` are dbt macros, so the value a comparison level will see is the
  value DuckDB computes from the rendered SQL; `tests/unit/dbt/harness.py` renders
  and runs them, and `RegexV1Parser` is S4.2's own oracle for `address_parse`. A
  second normalizer written here would be a second definition of S4.2 that no
  validator could see.
* **Survivorship coverage is computed, not asserted row by row.** S4.6 makes a
  chain a sequence of `ORDER BY` fragments, so "this rule decides here" is a
  property of the fixture and the config together. :func:`first_separating_rule`
  evaluates the configured chain against a persona's rows, and the coverage test
  asks only that each of the five rules is somewhere the first element able to
  separate -- so a config change that reorders a chain fails here rather than
  quietly leaving a rule with no deciding case (M11).

`truth.csv` and `traps.csv` are S8.2.1 ground truth: read here and by the S8.5
metrics, never fed to the pipeline, and `persona_id` is stripped from every input
row by the loader before `er ingest`.
"""

from __future__ import annotations

import csv
import re
import sys
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from itertools import combinations
from math import comb
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from helpers.scenario import load_scenario

from er.config.loader import load_config
from er.config.schema import CANONICAL_ATTRIBUTES, TERMINAL_SURVIVORSHIP_RULE, Config, SourceSpec
from er.entities.ids import record_key
from er.std.address import AddressComponents, RegexV1Parser

REPO_ROOT = Path(__file__).resolve().parents[3]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"

#: `configs/test.yaml` is the document S6 says the fixtures and CI use verbatim;
#: `configs/default.yaml` is the reference a deployment copies. The headers are
#: derived from the first, and the second is asserted to carry the same shape.
TEST_CONFIG_PATH = REPO_ROOT / "configs" / "test.yaml"
DEFAULT_CONFIG_PATH = REPO_ROOT / "configs" / "default.yaml"

SCENARIO_NAME = "base_10"

#: The truth column S8.2 puts last on every input row. It never reaches the
#: pipeline, which is why it is named here and in no `sources.columns` mapping.
PERSONA_COLUMN = "persona_id"

#: Every dbt invocation carries a `run_id` (S6); no macro read here uses it. A
#: literal rather than a fresh ULID, so a render is reproducible across sessions.
RUN_ID = "01JZZZZZZZZZZZZZZZZZZZZZZZ"

#: S8.2's ground-truth counts, restated as the shape the corpus must have.
EXPECTED_RECORDS = 23
EXPECTED_PERSONAS = 10
EXPECTED_PERSONA_SIZES = (4, 3, 3, 2, 2, 2, 2, 2, 2, 1)
EXPECTED_TRUE_PAIRS = 18

#: The survivorship attributes whose `validated` rule has an input (S4.6): only
#: `email` and `phone_e164` carry an `<attr>_valid` column on `int_std_records`.
VALID_COLUMN = {"email": "email_valid", "phone_e164": "phone_valid"}

#: The five S4.6 rules a chain may name, each of which M11 requires a deciding
#: case for. The terminal `record_key ASC` is not one of them: it is what fires
#: when none of these separates, and its case is the tiebreak row.
SURVIVORSHIP_RULES = ("source_priority", "recency", "frequency", "completeness", "validated")


def _import_macro_harness() -> ModuleType:
    """Import `tests/unit/dbt/harness.py`, whose directory is not on the path here.

    `tests/unit/dbt/` carries no `__init__.py`, so pytest puts it on ``sys.path``
    only while collecting that directory -- which this ticket's verify command,
    naming this file alone, does not do. The bootstrap therefore has to run before
    the import, and a bare ``sys.path`` statement followed by an import is an E402
    this ticket may not suppress, so the two happen together in here exactly as
    ``scripts/validate_fixtures.py`` does it.
    """
    entry = str(REPO_ROOT / "tests" / "unit" / "dbt")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__("harness", fromlist=["_"])


@dataclass(frozen=True)
class Record:
    """One input row, raw and standardized, with the truth label beside it."""

    source_system: str
    source_record_id: str
    persona_id: str
    priority_rank: int
    #: The raw field values, keyed by CANONICAL ATTRIBUTE rather than by source
    #: column, so a trap assertion reads the same in all three sources.
    raw: Mapping[str, str]
    updated_at: str
    given_name: str | None
    family_name: str | None
    email: str | None
    email_valid: bool | None
    phone_e164: str | None
    phone_valid: bool | None
    address: AddressComponents
    addr_city: str | None
    addr_region: str | None
    addr_postal: str | None
    birth_date: str | None

    @property
    def key(self) -> str:
        return record_key(self.source_system, self.source_record_id)

    @property
    def ref(self) -> tuple[str, str]:
        """The `(source_system, source_record_id)` pair truth.csv and traps.csv cite."""
        return (self.source_system, self.source_record_id)

    @property
    def addr_components(self) -> tuple[str | None, ...]:
        """The six S5 `addr_*` values: three parsed from `address_line`, three mapped."""
        return (
            self.address.addr_number,
            self.address.addr_street,
            self.address.addr_unit,
            self.addr_city,
            self.addr_region,
            self.addr_postal,
        )

    def value(self, attribute: str) -> str | None:
        """The value the S4.6 survivorship chain for ``attribute`` ranks."""
        if attribute == "address":
            parts = self.addr_components
            return None if None in parts else "|".join(str(part) for part in parts)
        value: str | None = getattr(self, attribute)
        return value


# --------------------------------------------------------------------------- #
# reading the fixture
# --------------------------------------------------------------------------- #


def read_csv(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as handle:
        return list(csv.DictReader(handle))


def header_of(path: Path) -> str:
    return path.read_text(encoding="utf-8").splitlines()[0]


def derived_header(spec: SourceSpec) -> str:
    """S8.2's header for one source, derived from S6 and from nothing else.

    `record_id_column`, then the `columns` values in canonical-attribute order,
    then `updated_at_column`, then the truth column.
    """
    fields = (
        spec.record_id_column,
        *(spec.columns[attribute] for attribute in CANONICAL_ATTRIBUTES),
        spec.updated_at_column,
        PERSONA_COLUMN,
    )
    return ",".join(fields)


def _fenced_pairs(body: str) -> dict[str, str]:
    """A ``name`` / ``value`` fenced block, as S8.2 and FORMAT.md both write one."""
    pairs: dict[str, str] = {}
    pending: str | None = None
    for line in body.splitlines():
        stripped = line.strip()
        if not stripped:
            pending = None
            continue
        if pending is None:
            pending = stripped
        else:
            pairs[pending] = stripped
            pending = None
    return pairs


def s8_2_input_headers() -> dict[str, str]:
    """S8.2's literal `base_10` header block, as ``file name -> header row``."""
    block = re.search(
        r"\*\*`base_10` input headers\.\*\*.*?```\n(?P<body>.*?)```",
        DESIGN_DOC.read_text(encoding="utf-8"),
        re.S,
    )
    assert block is not None, "S8.2 has no fenced `base_10` input-header block"
    return _fenced_pairs(block.group("body"))


def s8_2_trap_names() -> tuple[str, ...]:
    """The eight trap names of S8.2's designed-traps table, slugified.

    Read from the spec rather than transcribed, so a ninth trap row fails
    `traps.csv` on the next run instead of being one nobody indexed.
    """
    block = re.search(
        r"\*\*`base_10` designed traps\.\*\*[^\n]*\n\n(?P<body>(?:\|[^\n]*\n)+)",
        DESIGN_DOC.read_text(encoding="utf-8"),
    )
    assert block is not None, "S8.2 has no designed-traps table"
    rows = block.group("body").splitlines()
    names = []
    for row in rows[2:]:  # row 0 is the header, row 1 the `|---|` separator
        cell = row.split("|")[1].strip()
        names.append(re.sub(r"[^a-z0-9]+", "_", cell.lower()).strip("_"))
    return tuple(names)


# --------------------------------------------------------------------------- #
# fixtures
# --------------------------------------------------------------------------- #


@pytest.fixture(scope="module")
def config() -> Config:
    return load_config(TEST_CONFIG_PATH)


@pytest.fixture(scope="module")
def harness(config: Config) -> Iterator[Any]:
    """The macro tree, rendered against the vars the CLI would pass (S8.1)."""
    from er.dbt_runner import render_dbt_vars

    macro_harness = _import_macro_harness().MacroHarness(vars=render_dbt_vars(config, RUN_ID))
    try:
        yield macro_harness
    finally:
        macro_harness.close()


@pytest.fixture(scope="module")
def input_paths(config: Config) -> dict[str, Path]:
    """The scenario's `base/` delivery, keyed by source name."""
    scenario = load_scenario(SCENARIO_NAME)
    paths = dict(scenario.inputs_for("base"))
    assert set(paths) == set(config.sources), (
        f"base/ delivers {sorted(paths)}; S6 declares sources {sorted(config.sources)}"
    )
    return paths


@pytest.fixture(scope="module")
def truth_rows() -> list[dict[str, str]]:
    return read_csv(load_scenario(SCENARIO_NAME).truth["truth.csv"])


@pytest.fixture(scope="module")
def trap_rows() -> list[dict[str, str]]:
    return read_csv(load_scenario(SCENARIO_NAME).truth["traps.csv"])


@pytest.fixture(scope="module")
def records(config: Config, harness: Any, input_paths: dict[str, Path]) -> tuple[Record, ...]:
    """Every input row, standardized through the pipeline's own macros."""
    parser = RegexV1Parser()

    def scalar(macro: str, value: str) -> Any:
        return harness.eval_macro(macro, value)[0]["value"]

    built: list[Record] = []
    for source_system in sorted(config.sources):
        spec = config.sources[source_system]
        for row in read_csv(input_paths[source_system]):
            raw = {attribute: row[spec.columns[attribute]] for attribute in CANONICAL_ATTRIBUTES}
            email = harness.eval_macro("email_norm", raw["email"])[0]
            phone = harness.eval_macro("phone_e164", raw["phone"])[0]
            birth_date = raw["birth_date"].strip()
            built.append(
                Record(
                    source_system=source_system,
                    source_record_id=row[spec.record_id_column],
                    persona_id=row[PERSONA_COLUMN],
                    priority_rank=spec.priority_rank,
                    raw=raw,
                    updated_at=row[spec.updated_at_column],
                    given_name=scalar("name_norm", raw["given_name"]),
                    family_name=scalar("name_norm", raw["family_name"]),
                    email=email["email"],
                    email_valid=email["email_valid"],
                    phone_e164=phone["phone_e164"],
                    phone_valid=phone["phone_valid"],
                    address=parser.parse(raw["address_line"]),
                    addr_city=scalar("lowercase_trim", raw["addr_city"]),
                    addr_region=scalar("lowercase_trim", raw["addr_region"]),
                    addr_postal=scalar("lowercase_trim", raw["addr_postal"]),
                    birth_date=(
                        datetime.strptime(birth_date, spec.date_format).date().isoformat()
                        if birth_date
                        else None
                    ),
                )
            )
    return tuple(built)


@pytest.fixture(scope="module")
def personas(records: tuple[Record, ...]) -> dict[str, tuple[Record, ...]]:
    grouped: dict[str, list[Record]] = {}
    for record in records:
        grouped.setdefault(record.persona_id, []).append(record)
    return {persona: tuple(rows) for persona, rows in sorted(grouped.items())}


@pytest.fixture(scope="module")
def by_ref(records: tuple[Record, ...]) -> dict[tuple[str, str], Record]:
    return {record.ref: record for record in records}


# --------------------------------------------------------------------------- #
# S4.6 chain evaluation
# --------------------------------------------------------------------------- #


def _rule_key(rule: str, record: Record, attribute: str, rows: Sequence[Record]) -> Any:
    """The S4.6 `ORDER BY` fragment for ``rule``, as an ascending sort key.

    Every fragment is inverted here so that "lower is better" holds uniformly; the
    tests only ever ask whether a rule takes more than one value over a persona's
    rows, and that question is invariant under the inversion.
    """
    value = record.value(attribute)
    if rule == "source_priority":
        return record.priority_rank
    if rule == "recency":
        return record.updated_at
    if rule == "frequency":
        return -sum(1 for other in rows if other.value(attribute) == value)
    if rule == "completeness":
        return (value is None, -len(value) if value is not None else 0)
    if rule == "validated":
        valid = getattr(record, VALID_COLUMN[attribute])
        return {True: 0, False: 1, None: 2}[valid]
    raise AssertionError(f"S4.6 names no survivorship rule {rule!r}")


def first_separating_rule(
    chain: Sequence[str], rows: Sequence[Record], attribute: str
) -> str | None:
    """The first element of ``chain`` that takes more than one value over ``rows``.

    ``None`` means no rule in the chain separates them, so the mandatory terminal
    `record_key ASC` decides and `golden_lineage.rule` is `tiebreak_deterministic`.
    """
    for rule in chain:
        if rule == TERMINAL_SURVIVORSHIP_RULE:
            break
        if len({_rule_key(rule, record, attribute, rows) for record in rows}) > 1:
            return rule
    return None


def deciding_rules(
    config: Config, personas: Mapping[str, tuple[Record, ...]]
) -> dict[str, list[tuple[str, str]]]:
    """`rule -> the (persona, attribute) pairs it is the first separator of`."""
    decided: dict[str, list[tuple[str, str]]] = {}
    for persona, rows in personas.items():
        if len(rows) < 2:
            continue  # one row is its own winner; no rule is asked anything
        for attribute, chain in sorted(config.survivorship.items()):
            rule = first_separating_rule(chain, rows, attribute)
            if rule is not None:
                decided.setdefault(rule, []).append((persona, attribute))
    return decided


def shares_exact_key(left: Record, right: Record) -> bool:
    """Whether two records share an exact normalized email or an exact E.164 phone."""
    return (left.email is not None and left.email == right.email) or (
        left.phone_e164 is not None and left.phone_e164 == right.phone_e164
    )


# --------------------------------------------------------------------------- #
# AC1
# --------------------------------------------------------------------------- #


def test_headers_are_derived_from_config(config: Config, input_paths: dict[str, Path]) -> None:
    spec_headers = s8_2_input_headers()
    assert set(spec_headers) == {f"{source}.csv" for source in config.sources}

    for source_system, path in sorted(input_paths.items()):
        derived = derived_header(config.sources[source_system])
        assert header_of(path) == derived, source_system
        assert spec_headers[f"{source_system}.csv"] == derived, source_system

    # The reference document a deployment copies carries the same three sources, so
    # the header derived from either config is the same header (V11 on both).
    default = load_config(DEFAULT_CONFIG_PATH)
    assert set(default.sources) == set(config.sources)
    for source_system, spec in sorted(config.sources.items()):
        assert derived_header(default.sources[source_system]) == derived_header(spec)
        assert default.sources[source_system].date_format == spec.date_format
        assert default.sources[source_system].priority_rank == spec.priority_rank


# --------------------------------------------------------------------------- #
# AC2
# --------------------------------------------------------------------------- #


def test_truth_counts_23_records_10_personas_18_pairs(
    records: tuple[Record, ...],
    personas: dict[str, tuple[Record, ...]],
    truth_rows: list[dict[str, str]],
) -> None:
    assert len(records) == EXPECTED_RECORDS
    assert len({record.ref for record in records}) == EXPECTED_RECORDS

    # truth.csv and the inputs are two independent statements of the same labelling.
    labelled = {
        (row["source_system"], row["source_record_id"]): row[PERSONA_COLUMN] for row in truth_rows
    }
    assert len(labelled) == len(truth_rows) == EXPECTED_RECORDS
    assert labelled == {record.ref: record.persona_id for record in records}

    assert len(personas) == EXPECTED_PERSONAS
    sizes = tuple(sorted((len(rows) for rows in personas.values()), reverse=True))
    assert sizes == EXPECTED_PERSONA_SIZES

    true_pairs = sum(comb(len(rows), 2) for rows in personas.values())
    assert true_pairs == EXPECTED_TRUE_PAIRS
    assert comb(EXPECTED_RECORDS, 2) == 253


# --------------------------------------------------------------------------- #
# AC3
# --------------------------------------------------------------------------- #


def test_trap_index_is_complete_and_constructions_hold(
    records: tuple[Record, ...],
    trap_rows: list[dict[str, str]],
    by_ref: dict[tuple[str, str], Record],
) -> None:
    traps: dict[str, list[Record]] = {}
    for row in trap_rows:
        ref = (row["source_system"], row["source_record_id"])
        assert ref in by_ref, f"traps.csv cites {ref}, which no input row carries"
        traps.setdefault(row["trap"], []).append(by_ref[ref])

    assert sorted(traps) == sorted(s8_2_trap_names())

    robert, bob = sorted(traps["nickname_pair"], key=lambda record: record.source_system)
    assert (robert.source_system, robert.given_name, robert.family_name) == (
        "crm",
        "robert",
        "chen",
    )
    assert (bob.source_system, bob.given_name, bob.family_name) == ("webforms", "bob", "chen")
    assert robert.phone_e164 is not None
    assert robert.phone_e164 == bob.phone_e164
    assert robert.raw["phone"] != bob.raw["phone"], (
        "one spelling proves nothing about normalization"
    )

    krystal_billing, krystal_crm = sorted(traps["typo_surname"], key=lambda r: r.source_system)
    assert (krystal_billing.source_system, krystal_billing.family_name) == ("billing", "nowakowski")
    assert (krystal_crm.source_system, krystal_crm.family_name) == ("crm", "nowakowsky")
    assert krystal_billing.birth_date == krystal_crm.birth_date
    assert krystal_billing.addr_postal == krystal_crm.addr_postal

    left, right = traps["shared_household"]
    assert left.persona_id != right.persona_id
    assert left.raw["address_line"] == right.raw["address_line"]
    assert left.addr_components == right.addr_components
    assert left.family_name != right.family_name
    assert left.given_name != right.given_name
    assert left.birth_date != right.birth_date
    assert not shares_exact_key(left, right)

    placeholder = "test@test.com"
    carrying = [record for record in records if record.raw["email"] == placeholder]
    assert len(carrying) == 2
    assert {record.persona_id for record in carrying} == {
        record.persona_id for record in traps["placeholder_email"]
    }
    assert len({record.persona_id for record in carrying}) == 2
    # S4.2 splits the two vocabularies: `email_norm` nulls the placeholder, and it
    # is NULL rather than false because a placeholder is no evidence, not evidence
    # of an unparsable address.
    assert all(record.email is None and record.email_valid is None for record in carrying)

    empty = [record for record in records if record.raw["email"] == ""]
    assert len(empty) == 4
    assert {record.ref for record in empty} == {record.ref for record in traps["missing_emails"]}
    assert all(record.email is None for record in empty)

    drifted = traps["drifted_phones"]
    assert len(drifted) == 3
    assert len({record.persona_id for record in drifted}) == 1
    assert {record.raw["phone"] for record in drifted} == {
        "(415) 555-0132",
        "415-555-0132",
        "+14155550132",
    }
    assert {record.phone_e164 for record in drifted} == {"+14155550132"}


# --------------------------------------------------------------------------- #
# AC4
# --------------------------------------------------------------------------- #


def test_gray_band_candidate_is_unique_and_cross_persona(
    records: tuple[Record, ...],
    trap_rows: list[dict[str, str]],
    by_ref: dict[tuple[str, str], Record],
) -> None:
    def is_candidate(left: Record, right: Record) -> bool:
        if left.family_name is None or right.family_name is None or left.addr_postal is None:
            return False
        return (
            left.addr_postal == right.addr_postal
            and left.family_name[:4] == right.family_name[:4]
            and left.birth_date != right.birth_date
            and not shares_exact_key(left, right)
        )

    candidates = [pair for pair in combinations(records, 2) if is_candidate(*pair)]
    assert len(candidates) == 1, [(left.key, right.key) for left, right in candidates]

    left, right = candidates[0]
    assert left.persona_id != right.persona_id

    cited = {
        (row["source_system"], row["source_record_id"])
        for row in trap_rows
        if row["trap"] == "gray_band_pair"
    }
    assert {left.ref, right.ref} == cited
    assert all(by_ref[ref].addr_postal == left.addr_postal for ref in cited)


# --------------------------------------------------------------------------- #
# AC5
# --------------------------------------------------------------------------- #


def test_survivorship_rule_coverage_and_tiebreak_row(
    config: Config, personas: dict[str, tuple[Record, ...]]
) -> None:
    decided = deciding_rules(config, personas)
    missing = [rule for rule in SURVIVORSHIP_RULES if not decided.get(rule)]
    assert not missing, (
        f"no (persona, attribute) is decided by {missing}; every S4.6 rule needs a deciding case"
    )

    tiebreak = [
        (persona, left, right)
        for persona, rows in personas.items()
        for left, right in combinations(rows, 2)
        if left.source_system == right.source_system
        and left.priority_rank == right.priority_rank
        and left.updated_at == right.updated_at
        and left.given_name is not None
        and right.given_name is not None
        and left.given_name != right.given_name
        and len(left.given_name) == len(right.given_name)
    ]
    assert len(tiebreak) == 1, [persona for persona, _, _ in tiebreak]

    persona, left, right = tiebreak[0]
    # The terminal `record_key ASC` is what is left once no rule separates them,
    # which is the row S4.6 labels `tiebreak_deterministic`.
    chain = config.survivorship["given_name"]
    assert first_separating_rule(chain, (left, right), "given_name") is None
    assert (persona, "given_name") not in [pair for pairs in decided.values() for pair in pairs]


# --------------------------------------------------------------------------- #
# AC6
# --------------------------------------------------------------------------- #


def test_recency_never_decided_by_ingested_at(
    config: Config, personas: dict[str, tuple[Record, ...]]
) -> None:
    decided = deciding_rules(config, personas).get("recency", [])
    assert decided, "no attribute is decided by recency, so this guard would be vacuous"

    for persona, attribute in decided:
        rows = personas[persona]
        stamps = [record.updated_at for record in rows]
        assert all(stamps), f"{persona}/{attribute} would fall back to the volatile ingested_at"
        assert len(set(stamps)) == len(stamps), f"{persona}/{attribute} has a repeated updated_at"


# --------------------------------------------------------------------------- #
# AC7
# --------------------------------------------------------------------------- #


def test_two_record_personas_have_an_exact_key(personas: dict[str, tuple[Record, ...]]) -> None:
    pairs = [rows for rows in personas.values() if len(rows) == 2]
    assert len(pairs) == EXPECTED_PERSONA_SIZES.count(2)

    for left, right in pairs:
        assert shares_exact_key(left, right), (
            f"{left.key} / {right.key} is a 2-record persona connected only by a "
            "name or postal similarity; a missed edge there splits the persona and "
            "fails T-MATCH-1b (S8.2)"
        )


# --------------------------------------------------------------------------- #
# AC8
# --------------------------------------------------------------------------- #


def test_addresses_parse_and_ids_have_no_colon(records: tuple[Record, ...]) -> None:
    parser = RegexV1Parser()
    for record in records:
        assert ":" not in record.source_record_id, record.source_record_id
        assert record.key == f"{record.source_system}:{record.source_record_id}"

        components = record.address
        assert None not in record.addr_components, f"{record.key}: {record.raw['address_line']!r}"

        rendered = f"{components.addr_number} {components.addr_street} {components.addr_unit}"
        assert " ".join(record.raw["address_line"].split()).lower() == rendered, (
            f"{record.key}: the parse leaves text over"
        )
        assert parser.parse(rendered) == components
