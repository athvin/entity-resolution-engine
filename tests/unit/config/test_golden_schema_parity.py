"""S6.1 V2: the `survivorship:` key set and `GOLDEN_SURVIVABLE_COLUMNS` are one fact.

S5 says it normatively — "that survivable column set is exactly the key set of
`survivorship:` in S6, with `address` expanding to the six `addr_*` columns" — and V2 is
the validator that enforces it. This file is the test that keeps V2 honest.

**Why both directions, and why by perturbation.** A parity test that only reads the
committed config passes on any two sets that happen to agree today, and would keep
passing after someone added a column to one side and forgot the other — which is the
single failure it exists to catch. So each direction is provoked: a config carrying a
`survivorship:` key with no matching golden column, and a config missing a key that a
golden column requires. If either perturbation loaded successfully, V2 would be dead
code and the drift it guards against would be silent.

**And why the constants are checked for a shared derivation, not merely equal values.**
Two independently-written tuples that agree today are exactly the shape of a bug that
appears the day S5 grows a column. `er.config.validators` therefore imports the address
composite and the lineage vocabulary from `er.lake.columns` rather than deriving them a
second time, and that import is asserted here as well as in
`tests/unit/test_columns.py` — from the config side, where a re-derivation would land.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, Final

import pytest
import yaml

from er.config import validators
from er.config.loader import ConfigValidationError, load_config
from er.lake.columns import (
    ADDRESS_ATTRIBUTE,
    ADDRESS_COMPOSITE_COLUMNS,
    GOLDEN_LINEAGE_ATTRIBUTES,
    GOLDEN_SURVIVABLE_COLUMNS,
)

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
TEST_CONFIG: Final = REPO_ROOT / "configs" / "test.yaml"

#: The failure key S6.1 gives V2, as `validators.py` spells it.
V2_KEY: Final = "survivorship.keyset"


def document() -> dict[str, Any]:
    """The committed S6 document, as plain data ready to perturb."""
    loaded = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict)
    return loaded


def write(tmp_path: Path, doc: dict[str, Any]) -> Path:
    path = tmp_path / "config.yaml"
    path.write_text(yaml.safe_dump(doc, sort_keys=False), encoding="utf-8")
    return path


def test_survivorship_keyset_equals_golden_survivable_columns() -> None:
    """AC8: the committed config satisfies the equality, expansion included."""
    chains = load_config(TEST_CONFIG).survivorship

    expanded = validators.expand_survivorship_keys(chains)
    assert expanded == frozenset(GOLDEN_SURVIVABLE_COLUMNS), (
        f"expanding the survivorship keys gives {sorted(expanded)}, which is not the "
        f"survivable column set {sorted(GOLDEN_SURVIVABLE_COLUMNS)}"
    )

    assert frozenset(chains) == frozenset(GOLDEN_LINEAGE_ATTRIBUTES), (
        "the config's survivorship keys are not golden_lineage's attribute vocabulary; "
        "S5 makes them the same six tokens"
    )
    assert ADDRESS_ATTRIBUTE in chains, (
        "`address` is the one survivorship key that is not itself a golden column, and "
        "it is missing"
    )
    for column in ADDRESS_COMPOSITE_COLUMNS:
        assert column not in chains, (
            f"{column} has its own survivorship chain. S4.6 decides the address ONCE and "
            "reads all six columns off the winning record; a per-column chain is the "
            "field-by-field assembly the rule forbids."
        )


def test_parity_fails_when_a_survivorship_key_has_no_golden_column(
    tmp_path: Path,
) -> None:
    """AC8, direction one: a key naming nothing survivable is refused."""
    doc = document()
    doc["survivorship"]["nickname"] = ["source_priority"]

    with pytest.raises(ConfigValidationError) as raised:
        load_config(write(tmp_path, doc))
    # The loader surfaces S6.1's failure KEY rather than the validator's prose, so the
    # key is what a caller can act on and the key is what this asserts. Checking for the
    # offending name would be checking the message text, which S6.1 does not pin.
    assert V2_KEY in str(raised.value), (
        f"the document was refused by something other than V2: {raised.value}"
    )


def test_parity_fails_when_a_golden_column_has_no_survivorship_key(
    tmp_path: Path,
) -> None:
    """AC8, direction two: dropping a key leaves a golden column nothing decides."""
    doc = document()
    removed = doc["survivorship"].pop("birth_date")
    assert removed, "the fixture config has no birth_date chain; the arm would be vacuous"

    with pytest.raises(ConfigValidationError) as raised:
        load_config(write(tmp_path, doc))
    assert V2_KEY in str(raised.value), (
        f"the document was refused by something other than V2: {raised.value}"
    )


def test_parity_fails_when_an_address_column_is_keyed_directly(tmp_path: Path) -> None:
    """AC8: `addr_postal` is a column, never a key — the composite is one decision."""
    doc = document()
    doc["survivorship"]["addr_postal"] = ["source_priority"]

    with pytest.raises(ConfigValidationError) as raised:
        load_config(write(tmp_path, doc))
    assert V2_KEY in str(raised.value), (
        f"the document was refused by something other than V2: {raised.value}"
    )


def test_validators_do_not_re_derive_the_vocabulary() -> None:
    """One fact, one derivation — checked from the side that would re-derive it.

    `validators.py`'s own docstring says V2 exists to keep the `survivorship:` key set
    and `GOLDEN_SURVIVABLE_COLUMNS` one fact. A validator that computed its own copy of
    the address composite would be the first place that fact could split, and the split
    would show up as a config the validator accepts and the mart cannot build.
    """
    assert validators.ADDRESS_COLUMNS is ADDRESS_COMPOSITE_COLUMNS
    assert validators.ADDRESS_KEY == ADDRESS_ATTRIBUTE
    assert validators.SURVIVORSHIP_KEYS == frozenset(GOLDEN_LINEAGE_ATTRIBUTES)

    source = (REPO_ROOT / "src" / "er" / "config" / "validators.py").read_text("utf-8")
    assert 'startswith("addr_")' not in source, (
        "validators.py derives the address composite itself; it must import "
        "ADDRESS_COMPOSITE_COLUMNS from er.lake.columns instead"
    )
