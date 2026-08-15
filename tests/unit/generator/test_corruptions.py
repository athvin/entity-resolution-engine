"""Per-source corruption profiles (DesignDoc.md S10.1, S8.2, S6, S4.2).

`corruptions.py` is the only place the generated corpus stops being ground truth, so
every claim the benchmark makes about match quality is a claim about what this module
did. Four properties are load-bearing and each is asserted against an external
authority rather than against a restatement of the code:

* **A nickname is justified by the ER-039 seed or it is not emitted.** `variant_match`
  (S4.2) compares through `name_variants`, which is backed by
  `dbt/seeds/nickname_variants.csv`. A diminutive invented here would be a true pair
  the pipeline is structurally unable to find, and it would read in the S10.5 numbers
  as a recall regression with nothing in the pipeline to fix. The test reads the seed.
* **Only the three S8.2 phone surfaces are emitted, and they are one number.** S8.2's
  drifted-phone trap requires `(415) 555-0132`, `415-555-0132` and `+14155550132` to
  normalize together; a fourth surface would be a blocking-key split the fixture never
  exercises.
* **Every address parses under the v1 grammar.** S10.1 admits only patterns
  `RegexV1Parser` (S4.2) handles, and the parser is the authority: the test calls it
  rather than checking the corpus against a regex written here, so a parser change is
  a corpus failure rather than a silent divergence.
* **A profile that disagrees with the config is a load error.** S6 owns the source
  vocabulary; `load_profiles` is where that ownership is enforced.

Nothing here opens a connection, spawns dbt or reads the lake.
"""

from __future__ import annotations

import ast
import re
import sys
from collections import defaultdict
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

from er.config.loader import load_config
from er.std.address import RegexV1Parser

REPO_ROOT = Path(__file__).resolve().parents[3]

#: S6 calls this "the file the fixtures and CI use verbatim".
TEST_CONFIG_PATH = REPO_ROOT / "configs" / "test.yaml"

GENERATOR_DIR = REPO_ROOT / "fixtures" / "generator"


def _import(module: str) -> ModuleType:
    """Import a `fixtures/generator` module, whose parent is not on the path.

    `fixtures/` is committed data rather than a distribution, so nothing installs it,
    and a bare `sys.path` statement followed by an import is an E402 this ticket may
    not suppress. The two therefore happen together in here, exactly as
    `tests/unit/generator/test_personas.py` reaches `personas.py`.
    """
    entry = str(REPO_ROOT / "fixtures")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(f"generator.{module}", fromlist=["_"])


corruptions = _import("corruptions")
personas_module = _import("personas")

CONFIG = load_config(TEST_CONFIG_PATH)
PROFILES = corruptions.load_profiles(CONFIG)

#: S6, M26: the seed is a config field, so the test reads it.
SEED: int = CONFIG.generator.seed

#: 400 personas over three sources is 1,200 records -- above AC4's thousand, and the
#: `smoke` persona count of S10.2, so the sample is a scale the board actually runs.
PERSONA_COUNT = 400
HOUSEHOLD_RATE = 0.1

#: The three surfaces of S8.2's drifted-phone trap, as patterns. Written here rather
#: than imported so the test is an independent statement of the shape: a change to
#: `render_phone` that also changed a shared constant would otherwise pass.
PHONE_PATTERNS = {
    "e164": re.compile(r"^\+1[0-9]{10}$"),
    "dashed": re.compile(r"^[0-9]{3}-[0-9]{3}-[0-9]{4}$"),
    "parens": re.compile(r"^\([0-9]{3}\) [0-9]{3}-[0-9]{4}$"),
}

_CLOCK_MODULES = frozenset({"time", "calendar"})
_CLOCK_ATTRIBUTES = frozenset({"now", "today", "utcnow", "fromtimestamp", "time", "time_ns"})
_RANDOM_FUNCTIONS = frozenset(
    {
        "seed",
        "random",
        "randrange",
        "randint",
        "choice",
        "choices",
        "sample",
        "shuffle",
        "getrandbits",
        "uniform",
        "gauss",
    }
)


@pytest.fixture(scope="module")
def personas() -> list[Any]:
    """The draw every claim in this file is stated about."""
    return personas_module.generate_personas(SEED, PERSONA_COUNT, HOUSEHOLD_RATE)


def _records(personas: list[Any]) -> Iterator[tuple[Any, str, Any]]:
    """`(persona, source, corrupted record)` for every persona in every source.

    One record per persona per source: `emit.py` deals records unevenly across sources
    by design, and a sample that inherited that deal would test the three profiles at
    three different sample sizes for no reason.
    """
    for index, persona in enumerate(personas):
        for source, profile in PROFILES.items():
            rng = corruptions.record_rng(SEED, source, index, 0)
            yield persona, source, corruptions.corrupt_record(persona, profile, rng)


def _national_digits(phone: str) -> str:
    """The ten national digits of a US number, whatever surface carries them.

    Strip to digits, then drop the country code a `+1` surface carries and the other
    two do not. This is the reduction S4.2's `phone_e164` performs and the reason S8.2
    can assert that all three of its forms "normalize to `+16285550101`".
    """
    digits = "".join(character for character in phone if character.isdigit())
    return digits[1:] if len(digits) == 11 and digits.startswith("1") else digits


def test_nicknames_come_only_from_the_seed(personas: list[Any]) -> None:
    """AC5: every substituted given name is a pair in `nickname_variants.csv`."""
    pairs = corruptions.nickname_pairs()
    substitutions = 0
    for persona, source, record in _records(personas):
        emitted = record.given_name.lower()
        if emitted == persona.given_name:
            continue
        substitutions += 1
        assert frozenset({persona.given_name, emitted}) in pairs, (
            f"{source} emitted {emitted!r} for persona given name {persona.given_name!r}, "
            f"which is not a pair in {corruptions.NICKNAME_SEED_PATH}"
        )
    # Without this the assertion above passes vacuously against a generator that
    # substitutes nothing -- which is exactly the regression it exists to catch.
    assert substitutions > 0, "no nickname substitution fired; the axis is dead"


def test_phone_drift_forms_reduce_to_one_number(personas: list[Any]) -> None:
    """AC6: only the three S8.2 surfaces, and one number per persona."""
    by_persona: dict[str, set[str]] = defaultdict(set)
    forms_seen: set[str] = set()
    for persona, source, record in _records(personas):
        if not record.phone:
            continue  # per-field missingness; an empty field is not a surface
        matched = [name for name, pattern in PHONE_PATTERNS.items() if pattern.match(record.phone)]
        assert len(matched) == 1, (
            f"{source} emitted phone {record.phone!r}, which is not exactly one of the three "
            f"S8.2 surfaces {sorted(PHONE_PATTERNS)}"
        )
        forms_seen.add(matched[0])
        by_persona[persona.persona_id].add(_national_digits(record.phone))
        assert _national_digits(record.phone) == _national_digits(persona.phone)

    for persona_id, numbers in by_persona.items():
        assert len(numbers) == 1, f"{persona_id} drifted to {len(numbers)} distinct numbers"
    assert forms_seen == set(PHONE_PATTERNS), (
        f"only {sorted(forms_seen)} were emitted; every configured surface must appear "
        f"or the drift axis is not exercised"
    )


def test_every_address_parses_under_regex_v1(personas: list[Any]) -> None:
    """AC4: `RegexV1Parser` recovers a number and a street from every address."""
    parser = RegexV1Parser()
    stale = 0
    count = 0
    for persona, source, record in _records(personas):
        count += 1
        assert record.address_line, f"{source} emitted an empty address_line"
        components = parser.parse(record.address_line)
        assert components.addr_number is not None, (
            f"{source} emitted {record.address_line!r}, which the v1 parser gives no addr_number"
        )
        assert components.addr_street is not None, (
            f"{source} emitted {record.address_line!r}, which the v1 parser gives no addr_street"
        )
        if record.address_line.lower() != persona.address_line:
            stale += 1
    assert count >= 1000, f"AC4 is stated over a 1,000-record emission; this sample is {count}"
    assert stale > 0, "no stale address fired; the axis is dead"


def test_profile_validation_rejects_unknown_source_and_bad_rate(tmp_path: Path) -> None:
    """AC8: a source absent from `sources:` and an out-of-range rate both fail to load."""
    block = """
  typo_rate: 0.03
  nickname_rate: 0.1
  stale_address_rate: 0.04
  missing_rates:
    email: 0.02
  phone_form_weights:
    dashed: 1
"""
    known = "".join(f"{source}:{block}" for source in ("crm", "billing", "webforms"))

    unknown_source = tmp_path / "unknown-source.yaml"
    unknown_source.write_text(f"{known}salesforce:{block}", encoding="utf-8")
    with pytest.raises(ValueError, match=r"absent from sources:"):
        corruptions.load_profiles(CONFIG, unknown_source)

    bad_rate = tmp_path / "bad-rate.yaml"
    bad_rate.write_text(known.replace("typo_rate: 0.03", "typo_rate: 1.5", 1), encoding="utf-8")
    with pytest.raises(ValueError, match=r"rate must be within \[0, 1\], got 1\.5"):
        corruptions.load_profiles(CONFIG, bad_rate)

    negative_missing = tmp_path / "negative-missing.yaml"
    negative_missing.write_text(known.replace("email: 0.02", "email: -0.1", 1), encoding="utf-8")
    with pytest.raises(ValueError, match=r"missing_rates\.email: rate must be within"):
        corruptions.load_profiles(CONFIG, negative_missing)

    # The other direction: a configured source with no profile emits a whole system
    # with no drift, which reads as an unusually clean corpus rather than as a bug.
    absent = tmp_path / "absent-source.yaml"
    absent.write_text(f"crm:{block}billing:{block}", encoding="utf-8")
    with pytest.raises(ValueError, match=r"have no profile"):
        corruptions.load_profiles(CONFIG, absent)


def test_the_generator_reads_no_clock_and_no_unseeded_rng() -> None:
    """S10.1 and the ticket's DoD: no `datetime.now`, no `time.time`, no bare `random.`.

    Asserted over the module's AST rather than over a passing run: a `datetime.now()`
    in a rarely taken branch would satisfy every value assertion in this file and
    break the corpus's cross-process byte equality months later.
    """
    modules = sorted(GENERATOR_DIR.glob("*.py"))
    assert len(modules) >= 5, f"expected the generator's modules under {GENERATOR_DIR}"
    for path in modules:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        for node in ast.walk(tree):
            if not isinstance(node, ast.Attribute) or not isinstance(node.value, ast.Name):
                continue
            qualified = f"{node.value.id}.{node.attr}"
            if node.value.id == "datetime" and node.attr in _CLOCK_ATTRIBUTES:
                pytest.fail(f"{path.name} reads a clock: {qualified}")
            if node.value.id in _CLOCK_MODULES and node.attr in _CLOCK_ATTRIBUTES:
                pytest.fail(f"{path.name} reads a clock: {qualified}")
            if node.value.id == "random" and node.attr in _RANDOM_FUNCTIONS:
                pytest.fail(f"{path.name} draws from the unseeded module RNG: {qualified}")
