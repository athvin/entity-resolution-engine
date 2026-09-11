"""base_10's committed golden and lineage expectations, machine-checked (S4.6, S8.2.1).

T-GOLD-1 (ER-091) is the arbiter between these expectations and the dbt survivorship
macros; this module is the fixture-authoring aid that proves the two committed files
are internally consistent and reproducible from the committed inputs, so a fixture
edit that silently drops a rule from coverage fails HERE — at the unit layer — rather
than as a confusing integration diff.

**The oracle is not a second survivorship implementation.** It reproduces the winner
and the deciding rule the way ER-087's macro attributes them — rank the entity's
member rows on the chain's total order, and credit the first rule whose sort key
differs between the rank-1 and the rank-2 row — over a deliberately minimal
standardizer that covers exactly base_10's fields. The macros remain authoritative;
where the two could disagree, T-GOLD-1 is where the disagreement is caught.

The `frequency` rule is reachable here, which the first attempt of this ticket
measured it was NOT under the old model: base_10's E1 now holds two crm records whose
given names differ in frequency, so `source_priority` ties between them and
`frequency` decides — the reachability the M4 model retrain and base_10 re-authoring
created.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import datetime
from pathlib import Path
from typing import Any, Final

import pytest

from er.config.loader import load_config
from er.config.schema import TERMINAL_SURVIVORSHIP_RULE
from er.std.address import RegexV1Parser

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
CONFIG_PATH: Final = REPO_ROOT / "configs" / "test.yaml"
BASE: Final = REPO_ROOT / "fixtures" / "static" / "base_10"
GOLDEN_CSV: Final = BASE / "expected" / "base" / "golden.csv"
LINEAGE_CSV: Final = BASE / "expected" / "base" / "lineage.csv"
MEMBERSHIP_CSV: Final = BASE / "expected" / "base" / "membership.csv"

GOLDEN_HEADER: Final = (
    "entity_label,given_name,family_name,email,phone_e164,addr_number,addr_street,"
    "addr_unit,addr_city,addr_region,addr_postal,birth_date,survivorship_version"
).split(",")
LINEAGE_HEADER: Final = (
    "entity_label,attribute,record_key,source_system,source_record_id,rule,survivorship_version"
).split(",")

#: The six-token closed vocabulary S5's `golden_lineage.rule` comment pins (S4.6).
RULE_VOCABULARY: Final = frozenset(
    {
        "source_priority",
        "recency",
        "frequency",
        "completeness",
        "validated",
        "tiebreak_deterministic",
    }
)

#: The six lineage attributes: the five scalar ones plus the single `address` token
#: standing in for all six `addr_*` columns (S4.6).
LINEAGE_ATTRIBUTES: Final = (
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "address",
    "birth_date",
)

NULL_TOKEN: Final = "\\N"

EXPECTED_ENTITIES: Final = 10
EXPECTED_RECORDS: Final = 23
EXPECTED_TRUE_PAIRS: Final = 18
TERMINAL_RULE: Final = "tiebreak_deterministic"


def _rows(path: Path) -> tuple[list[str], list[list[str]]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    return lines[0].split(","), [line.split(",") for line in lines[1:] if line]


def _dicts(path: Path) -> list[dict[str, str]]:
    header, rows = _rows(path)
    return [dict(zip(header, row, strict=True)) for row in rows]


# --------------------------------------------------------------------------- #
# The minimal standardizer — exactly base_10's fields, no more (S4.2).
# --------------------------------------------------------------------------- #

_PLACEHOLDERS: Final = frozenset({"test@test.com", "noreply@example.com"})
_PARSER: Final = RegexV1Parser()


def _norm_name(value: str) -> str | None:
    value = value.strip().lower()
    return value or None


def _norm_email(value: str) -> str | None:
    value = value.strip().lower()
    if not value or value in _PLACEHOLDERS:
        return None
    return value


def _norm_phone(value: str) -> str | None:
    digits = "".join(c for c in value if c.isdigit())
    if not digits:
        return None
    if len(digits) == 10:
        return f"+1{digits}"
    if len(digits) == 11 and digits.startswith("1"):
        return f"+{digits}"
    return f"+{digits}"


def _norm_date(value: str, date_format: str) -> str | None:
    value = value.strip()
    if not value:
        return None
    return datetime.strptime(value, date_format).date().isoformat()


class Record:
    """One standardized member row, in the shape the oracle ranks."""

    def __init__(self, source: str, spec: Any, raw: Mapping[str, str], priority_rank: int) -> None:
        columns = spec.columns
        self.source_system = source
        self.source_record_id = raw[spec.record_id_column]
        self.record_key = f"{source}:{self.source_record_id}"
        self.priority_rank = priority_rank
        self.updated_at = raw[spec.updated_at_column].strip()
        self.given_name = _norm_name(raw[columns["given_name"]])
        self.family_name = _norm_name(raw[columns["family_name"]])
        self.email = _norm_email(raw[columns["email"]])
        self.phone_e164 = _norm_phone(raw[columns["phone"]])
        self.birth_date = _norm_date(raw[columns["birth_date"]], spec.date_format)
        components = _PARSER.parse(raw[columns["address_line"]])
        self.addr_number = components.addr_number
        self.addr_street = components.addr_street
        self.addr_unit = components.addr_unit
        self.addr_city = _norm_name(raw[columns["addr_city"]])
        self.addr_region = _norm_name(raw[columns["addr_region"]])
        self.addr_postal = raw[columns["addr_postal"]].strip() or None
        self.email_valid = self.email is not None
        self.phone_valid = self.phone_e164 is not None

    ADDRESS_COLUMNS: Final = (
        "addr_number",
        "addr_street",
        "addr_unit",
        "addr_city",
        "addr_region",
        "addr_postal",
    )

    def value(self, attribute: str) -> Any:
        if attribute == "address":
            return tuple(getattr(self, column) for column in self.ADDRESS_COLUMNS)
        return getattr(self, attribute)

    def scalar_value(self, attribute: str) -> str | None:
        return None if attribute == "address" else getattr(self, attribute)


def _load_records() -> dict[str, tuple[Record, ...]]:
    """`record_key -> Record`, grouped later by the committed membership."""
    cfg = load_config(CONFIG_PATH)
    records: dict[str, Record] = {}
    for source, spec in cfg.sources.items():
        header, rows = _rows(BASE / "base" / f"{source}.csv")
        for row in rows:
            raw = dict(zip(header, row, strict=True))
            record = Record(source, spec, raw, spec.priority_rank)
            records[record.record_key] = record
    return {key: (value,) for key, value in records.items()}


def _entities() -> dict[str, list[Record]]:
    """`entity_label -> member Records`, from the committed membership and inputs."""
    records = {key: value[0] for key, value in _load_records().items()}
    groups: dict[str, list[Record]] = {}
    for row in _dicts(MEMBERSHIP_CSV):
        key = f"{row['source_system']}:{row['source_record_id']}"
        groups.setdefault(row["entity_label"], []).append(records[key])
    return groups


# --------------------------------------------------------------------------- #
# The oracle: winner + deciding rule, ER-087's rank-1-vs-rank-2 attribution.
# --------------------------------------------------------------------------- #


def _rule_key(rule: str, record: Record, attribute: str, members: Sequence[Record]) -> Any:
    """Each S4.6 rule's sort key, inverted so 'lower is better' holds uniformly."""
    value = record.value(attribute)
    if rule == "source_priority":
        return record.priority_rank
    if rule == "recency":
        return datetime(9999, 12, 31) - datetime.strptime(record.updated_at, "%Y-%m-%d %H:%M:%S")
    if rule == "frequency":
        return -sum(1 for other in members if other.value(attribute) == value)
    if rule == "completeness":
        length = 0
        if attribute == "address":
            length = sum(len(part) for part in value if part)
            present = any(part for part in value)
        else:
            present = value is not None
            length = len(value) if value is not None else 0
        return (not present, -length)
    if rule == "validated":
        valid = record.email_valid if attribute == "email" else record.phone_valid
        return {True: 0, False: 1}[valid]
    raise AssertionError(f"S4.6 names no survivorship rule {rule!r}")


def oracle(attribute: str, chain: Sequence[str], members: Sequence[Record]) -> tuple[str, str]:
    """`(winning record_key, deciding rule)` for one entity's attribute (S4.6, ER-087)."""
    active = [rule for rule in chain if rule != TERMINAL_SURVIVORSHIP_RULE]

    def chain_key(record: Record) -> tuple[Any, ...]:
        return (
            *(_rule_key(rule, record, attribute, members) for rule in active),
            record.record_key,
        )

    ordered = sorted(members, key=chain_key)
    winner = ordered[0]
    if len(ordered) == 1:
        return winner.record_key, TERMINAL_RULE
    rank_2 = ordered[1]
    for rule in active:
        if _rule_key(rule, winner, attribute, members) != _rule_key(
            rule, rank_2, attribute, members
        ):
            return winner.record_key, rule
    return winner.record_key, TERMINAL_RULE


# --------------------------------------------------------------------------- #
# AC1
# --------------------------------------------------------------------------- #


def test_golden_csv_format_and_labels() -> None:
    """AC1: literal header, 10 rows, E1..E10 matching membership, \\N nulls."""
    header, rows = _rows(GOLDEN_CSV)
    assert header == GOLDEN_HEADER, f"golden.csv header is {header}"
    assert len(rows) == EXPECTED_ENTITIES, f"{len(rows)} golden rows, not {EXPECTED_ENTITIES}"
    assert rows == sorted(rows), "golden.csv is not byte-sorted on its column tuple"

    labels = {row[0] for row in rows}
    membership_labels = {row["entity_label"] for row in _dicts(MEMBERSHIP_CSV)}
    assert labels == membership_labels == {f"E{i}" for i in range(1, EXPECTED_ENTITIES + 1)}

    assert "assembled_at" not in header, "assembled_at is a VOLATILE_COLUMNS member (S8.2.1)"
    for row in rows:
        for value in row:
            assert not (
                value and value != NULL_TOKEN and value.count("-") == 4 and len(value) == 26
            ), f"a value looks like a ULID: {value!r}; entity_label must be symbolic"


# --------------------------------------------------------------------------- #
# AC2
# --------------------------------------------------------------------------- #


def test_lineage_grid_is_complete() -> None:
    """AC2: 60 rows, six-token attribute vocabulary, each attribute 10 times."""
    header, rows = _rows(LINEAGE_CSV)
    assert header == LINEAGE_HEADER, f"lineage.csv header is {header}"
    assert rows == sorted(rows), "lineage.csv is not byte-sorted"
    assert len(rows) == EXPECTED_ENTITIES * len(LINEAGE_ATTRIBUTES), f"{len(rows)} lineage rows"

    by_attribute: dict[str, int] = {}
    for row in rows:
        by_attribute[row[1]] = by_attribute.get(row[1], 0) + 1
    assert set(by_attribute) == set(LINEAGE_ATTRIBUTES), f"attributes are {set(by_attribute)}"
    assert set(by_attribute.values()) == {EXPECTED_ENTITIES}, by_attribute


# --------------------------------------------------------------------------- #
# AC3
# --------------------------------------------------------------------------- #


def test_lineage_records_are_entity_members() -> None:
    """AC3: every lineage record is a member of its entity, keyed canonically."""
    members: dict[str, set[str]] = {}
    for row in _dicts(MEMBERSHIP_CSV):
        members.setdefault(row["entity_label"], set()).add(
            f"{row['source_system']}:{row['source_record_id']}"
        )
    for row in _dicts(LINEAGE_CSV):
        record_key = f"{row['source_system']}:{row['source_record_id']}"
        assert row["record_key"] == record_key, (
            f"record_key {row['record_key']} != {record_key} (S5.0 composition)"
        )
        assert record_key in members[row["entity_label"]], (
            f"{record_key} is not a member of {row['entity_label']}"
        )


# --------------------------------------------------------------------------- #
# AC4
# --------------------------------------------------------------------------- #


def test_address_composite_comes_from_one_record() -> None:
    """AC4: the six addr_* in golden equal the standardized address of one record."""
    records = {key: value[0] for key, value in _load_records().items()}
    golden = {row["entity_label"]: row for row in _dicts(GOLDEN_CSV)}
    lineage = {
        (row["entity_label"], row["attribute"]): row["record_key"] for row in _dicts(LINEAGE_CSV)
    }
    address_columns = (
        "addr_number",
        "addr_street",
        "addr_unit",
        "addr_city",
        "addr_region",
        "addr_postal",
    )
    for label, row in golden.items():
        winner = records[lineage[(label, "address")]]
        for column in address_columns:
            expected = getattr(winner, column)
            got = None if row[column] == NULL_TOKEN else row[column]
            assert got == expected, (
                f"{label} {column}: golden={got!r} but the address winner "
                f"{winner.record_key} standardizes to {expected!r} (S4.6 composite rule)"
            )


# --------------------------------------------------------------------------- #
# AC5, AC6
# --------------------------------------------------------------------------- #


def test_rule_coverage_is_exactly_six_tokens() -> None:
    """AC5: set(lineage.rule) is exactly the six-token vocabulary; AC6: each rule is
    only on an attribute whose chain contains it."""
    cfg = load_config(CONFIG_PATH)
    rows = _dicts(LINEAGE_CSV)
    observed = {row["rule"] for row in rows}
    assert observed == RULE_VOCABULARY, (
        f"lineage rules are {sorted(observed)}, not the six-token vocabulary "
        f"{sorted(RULE_VOCABULARY)}"
    )

    for row in rows:
        rule = row["rule"]
        if rule == TERMINAL_RULE:
            continue  # the terminal element may decide any attribute
        chain = cfg.survivorship["address" if row["attribute"] == "address" else row["attribute"]]
        assert rule in chain, (
            f"{row['attribute']} carries rule {rule!r}, absent from its chain {list(chain)} "
            "(AC6): a rule may decide only where its chain names it"
        )


# --------------------------------------------------------------------------- #
# AC7
# --------------------------------------------------------------------------- #


def test_oracle_reproduces_winner_and_rule() -> None:
    """AC7: the oracle reproduces winner and rule for all 60 rows and every value."""
    cfg = load_config(CONFIG_PATH)
    entities = _entities()
    golden = {row["entity_label"]: row for row in _dicts(GOLDEN_CSV)}
    lineage = {(row["entity_label"], row["attribute"]): row for row in _dicts(LINEAGE_CSV)}

    mismatches: list[str] = []
    for label, members in entities.items():
        for attribute in LINEAGE_ATTRIBUTES:
            chain = cfg.survivorship["address" if attribute == "address" else attribute]
            winner_key, rule = oracle(attribute, list(chain), members)
            committed = lineage[(label, attribute)]
            if committed["record_key"] != winner_key:
                mismatches.append(
                    f"{label}/{attribute}: winner {committed['record_key']} != oracle {winner_key}"
                )
            if committed["rule"] != rule:
                mismatches.append(f"{label}/{attribute}: rule {committed['rule']} != oracle {rule}")
    assert not mismatches, "oracle disagrees with lineage.csv:\n  " + "\n  ".join(mismatches)

    # Every non-null golden scalar value equals the winning record's standardized value.
    records = {member.record_key: member for members in entities.values() for member in members}
    for label, row in golden.items():
        for attribute in ("given_name", "family_name", "email", "phone_e164", "birth_date"):
            winner = records[lineage[(label, attribute)]["record_key"]]
            expected = winner.scalar_value(attribute)
            got = None if row[attribute] == NULL_TOKEN else row[attribute]
            assert got == expected, (
                f"{label} {attribute}: golden={got!r} but the winner "
                f"{winner.record_key} standardizes to {expected!r}"
            )


# --------------------------------------------------------------------------- #
# AC8
# --------------------------------------------------------------------------- #


def test_truth_counts_unchanged() -> None:
    """AC8: base_10 still has 23 records over 10 personas with 18 true pairs."""
    from math import comb

    truth = _dicts(BASE / "truth.csv")
    assert len(truth) == EXPECTED_RECORDS
    personas: dict[str, int] = {}
    for row in truth:
        personas[row["persona_id"]] = personas.get(row["persona_id"], 0) + 1
    assert len(personas) == EXPECTED_ENTITIES
    assert sum(comb(size, 2) for size in personas.values()) == EXPECTED_TRUE_PAIRS


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
