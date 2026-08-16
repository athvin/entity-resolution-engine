"""V9 and V10 of S6.1, from the angle the training sequence needs them (S4.3.2).

`tests/unit/test_config_schema.py` walks S6.1 as a table of validators and owns the
"each rule has a unit test" requirement. This file is not a second copy of that walk:
it asserts the three rejections `er train` depends on being impossible to reach, and
asserts them as the *stage* experiences them — a document that loads is a document
whose `training:` block can drive :data:`er.matching.train.TRAIN_CALL_SEQUENCE` with
no argument left to a default.

That framing is why each case also checks the exit status. S4.0's table gives config
validation exit `2`, and the S4.3.2 arguments are exactly the ones M8 records as
having had no config home: a `u_seed` that fell back to Splink's ``None``, or a single
EM session, is not a stage failure an operator can retry — it is a document that must
be rejected before any lake connection is opened.
"""

from __future__ import annotations

import copy
from pathlib import Path
from typing import Any

import pytest
import yaml

from er.config.loader import ConfigValidationError, load_config
from er.errors import ExitCode, exit_code_for

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"


def document() -> dict[str, Any]:
    """A fresh, mutable copy of the shipped test document."""
    loaded = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict), "configs/test.yaml is not a mapping"
    return copy.deepcopy(loaded)


def write(tmp_path: Path, doc: dict[str, Any]) -> Path:
    path = tmp_path / "config.yaml"
    path.write_text(yaml.safe_dump(doc, sort_keys=False), encoding="utf-8")
    return path


def reject(tmp_path: Path, doc: dict[str, Any]) -> ConfigValidationError:
    """Load a document that must fail, and return the refusal."""
    with pytest.raises(ConfigValidationError) as raised:
        load_config(write(tmp_path, doc))
    return raised.value


def test_missing_u_seed_rejected(tmp_path: Path) -> None:
    """V10: `u_seed` is required with no default, and the refusal is exit 2.

    The negative case is the one that matters: Splink's own default for `seed` is
    ``None``, so a defaulted or optional `u_seed` would train successfully and produce
    a different `settings.json` every time. M8's second half is precisely that.
    """
    doc = document()
    del doc["training"]["u_seed"]

    error = reject(tmp_path, doc)

    assert error.errors[0].key == "training.u_seed.required"
    assert error.pointer == "/training/u_seed"
    assert error.code == int(ExitCode.CONFIG)
    assert exit_code_for(error) == 2

    # And the shipped value loads as the int the sequence passes as `seed=`.
    doc["training"]["u_seed"] = 20260101
    assert load_config(write(tmp_path, doc)).training.u_seed == 20260101


def test_em_blocking_rules_min_items(tmp_path: Path) -> None:
    """V9: fewer than two EM sessions, or no deterministic rule, is a rejection.

    Two sessions is not a style preference. EM does not estimate m for a column the
    session blocked on, so a single session leaves that column's m at its prior — the
    model would look trained and would score the blocked column as if it had never
    seen the corpus.
    """
    for rules in ([], ["l.email = r.email"]):
        doc = document()
        doc["training"]["em_blocking_rules"] = rules

        error = reject(tmp_path, doc)

        assert error.errors[0].key == "training.em_blocking_rules.min_items", rules
        assert error.code == int(ExitCode.CONFIG)

    # The same row covers the deterministic rules, which
    # `estimate_probability_two_random_records_match` cannot run without.
    doc = document()
    doc["training"]["deterministic_rules"] = []
    assert reject(tmp_path, doc).errors[0].key == "training.em_blocking_rules.min_items"

    # A third rule is accepted and stays in config order: S4.3.2 issues one session
    # per entry, so the list is the sequence.
    doc = document()
    doc["training"]["em_blocking_rules"] = [
        "l.email = r.email",
        "l.phone_e164 = r.phone_e164",
        "l.family_name = r.family_name and l.addr_postal = r.addr_postal",
    ]
    assert load_config(write(tmp_path, doc)).training.em_blocking_rules == [
        "l.email = r.email",
        "l.phone_e164 = r.phone_e164",
        "l.family_name = r.family_name and l.addr_postal = r.addr_postal",
    ]


@pytest.mark.parametrize("recall", [0.0, -0.1, 1.5])
def test_recall_bounds_rejected(tmp_path: Path, recall: float) -> None:
    """V10: `recall` outside `(0, 1]` is rejected, and `1.0` is inside it.

    `recall` is the denominator adjustment
    `estimate_probability_two_random_records_match` applies to the deterministic match
    count. Zero divides; above one claims the deterministic rules found more matches
    than exist. Neither raises in Splink — both produce a prior that is silently wrong.
    """
    doc = document()
    doc["training"]["recall"] = recall

    error = reject(tmp_path, doc)

    assert error.errors[0].key == "training.u_seed.required", (
        "S6.1 gives V10's three assertions one message key row; a different key here "
        "means the row was split without the spec being amended"
    )
    assert error.code == int(ExitCode.CONFIG)

    # The half-open bound is a bound, not a strict inequality on both sides.
    doc["training"]["recall"] = 1.0
    assert load_config(write(tmp_path, doc)).training.recall == 1.0

    # V10's third assertion shares the row, and shares this test's subject: an
    # `u_max_pairs` of zero samples nothing and leaves every u at its prior.
    doc["training"]["recall"] = 0.85
    doc["training"]["u_max_pairs"] = 0
    assert reject(tmp_path, doc).errors[0].key == "training.u_seed.required"
