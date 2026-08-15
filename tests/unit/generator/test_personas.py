"""`generate_personas` and the ground truth it produces (DesignDoc.md S10.1, S10.2, S6, S12).

The corpus this function emits is the input to T-TRAIN-1's byte-for-byte model
comparison (S8.3) and to every S10.4 benchmark fingerprint, so "it looked random
enough" is not a property any of them can be graded against. These eight tests pin
the ones that are:

* **Determinism is asserted across PROCESSES.** An in-process re-call proves almost
  nothing -- it shares the interpreter's hash seed, its module state and its `int`
  cache. The digest below is recomputed in a freshly spawned interpreter under a
  DIFFERENT `PYTHONHASHSEED`, which is what actually catches a `set` or `dict`
  iteration order leaking into the corpus (AC1).
* **The absence of a clock is checked in the source, not inferred from a passing
  run.** A `datetime.now()` in a rarely taken branch would pass every value
  assertion here and fail in CI six months later, so AC2 reads the module's AST.
* **The name skew is checked against the committed weight lists**, not against a
  hard-coded expectation: `data/family_names.csv` is the single statement of what
  the distribution should be, and a test repeating those numbers would let the two
  drift apart (AC4).

Nothing here opens a connection, spawns dbt or reads the lake. The one subprocess is
a bare interpreter running this file.
"""

from __future__ import annotations

import ast
import hashlib
import json
import os
import random
import re
import subprocess
import sys
from collections import Counter, defaultdict
from collections.abc import Sequence
from dataclasses import asdict
from datetime import date, datetime
from math import sqrt
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

from er.config.loader import load_config
from er.std.address import RegexV1Parser

REPO_ROOT = Path(__file__).resolve().parents[3]

#: `configs/test.yaml` is the document S6 says the fixtures and CI use verbatim.
TEST_CONFIG_PATH = REPO_ROOT / "configs" / "test.yaml"


def _import_personas() -> ModuleType:
    """Import `fixtures/generator/personas.py`, whose parent is not on the path.

    `fixtures/` is committed data rather than a distribution, so nothing installs
    it, and a bare `sys.path` statement followed by an import is an E402 this ticket
    may not suppress. The two therefore happen together in here, exactly as
    `tests/unit/fixtures/test_base_10.py` reaches the dbt macro harness.
    """
    entry = str(REPO_ROOT / "fixtures")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__("generator.personas", fromlist=["_"])


personas_module = _import_personas()
generate_personas = personas_module.generate_personas
family_names = personas_module.family_names
PERSONA_ID_PREFIX = personas_module.PERSONA_ID_PREFIX
PERSONA_ID_WIDTH = personas_module.PERSONA_ID_WIDTH

CONFIG = load_config(TEST_CONFIG_PATH)

#: S6, M26: the seed is a config field, so the test reads it rather than repeating
#: the literal `42` the generator would then be free to disagree with.
SEED: int = CONFIG.generator.seed

#: S10.2's `10k` row -- 4,000 personas -- because that is the scale S12 pins for the
#: committed fixture model, and the distribution claims below are stated at it.
PERSONA_COUNT = 4000
HOUSEHOLD_RATE = 0.1

#: The `date_format` of every configured source (S6), so AC8's "renderable under
#: both" is the set of formats the pipeline will actually ask for.
DATE_FORMATS = sorted({source.date_format for source in CONFIG.sources.values()})

#: What AC5 admits around the requested rate.
RATE_TOLERANCE = 0.02

#: The attributes a household's members MUST NOT share (S10.1). `given_name` is
#: absent on purpose: two people at one address sharing a first name is ordinary,
#: and it is these four that would make the pair a genuine duplicate.
HOUSEHOLD_DISJOINT_FIELDS = ("email", "phone", "family_name", "birth_date")

_CLOCK_MODULES = frozenset({"time", "calendar"})
_CLOCK_ATTRIBUTES = frozenset(
    {"now", "today", "utcnow", "fromtimestamp", "time", "time_ns", "monotonic", "perf_counter"}
)
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

#: Recomputes the digest in a bare interpreter. It loads THIS file by path rather
#: than reimplementing the serialisation, so the parent and the child cannot drift
#: into canonicalising the personas two different ways and calling that agreement.
_DIGEST_PROGRAM = """\
import importlib.util
import sys

path, seed, count, rate = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4])
spec = importlib.util.spec_from_file_location("er_persona_digest", path)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
sys.stdout.write(module.personas_digest(module.generate_personas(seed, count, rate)))
"""


def personas_digest(personas: Sequence[Any]) -> str:
    """SHA-256 over the canonical JSON serialisation of a draw.

    `sort_keys` and the tightest separators make the encoding a function of the
    values alone, and `default=str` renders a `date` as its ISO form -- so a change
    of field ORDER in the dataclass does not move the digest, while a change of any
    VALUE does.
    """
    payload = json.dumps(
        [asdict(persona) for persona in personas],
        sort_keys=True,
        separators=(",", ":"),
        default=str,
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


@pytest.fixture(scope="module")
def reference() -> list[Any]:
    """The draw every distribution claim in this file is stated about."""
    return generate_personas(SEED, PERSONA_COUNT, HOUSEHOLD_RATE)


def _address_key(persona: Any) -> tuple[str | None, ...]:
    """The six `addr_*` components -- what "the same address" means (S6)."""
    return (
        persona.addr_number,
        persona.addr_street,
        persona.addr_unit,
        persona.addr_city,
        persona.addr_region,
        persona.addr_postal,
    )


def _identity(persona: Any) -> tuple[Any, ...]:
    """Everything about a persona except the two ids, which are positional."""
    return (
        persona.given_name,
        persona.family_name,
        persona.email,
        persona.phone,
        persona.address_line,
        persona.addr_city,
        persona.addr_postal,
        persona.birth_date,
    )


def _is_random_call(node: ast.Call) -> bool:
    """Does this call reach into the `random` module's process-global state?"""
    func = node.func
    if isinstance(func, ast.Attribute):
        return isinstance(func.value, ast.Name) and func.value.id == "random"
    return isinstance(func, ast.Name) and func.id in _RANDOM_FUNCTIONS


def test_output_is_byte_identical_across_processes(reference: list[Any]) -> None:
    """AC1: the digest survives a fresh interpreter with a different hash seed."""
    assert len(reference) == PERSONA_COUNT
    expected = personas_digest(reference)

    environment = dict(os.environ)
    # The point of setting it: the child's `set` and `dict` iteration orders differ
    # from the parent's, so any dependence on them shows up here as a different
    # digest rather than as a one-in-many flake in a benchmark months later.
    environment["PYTHONHASHSEED"] = "1"
    completed = subprocess.run(
        [
            sys.executable,
            "-c",
            _DIGEST_PROGRAM,
            str(Path(__file__).resolve()),
            str(SEED),
            str(PERSONA_COUNT),
            str(HOUSEHOLD_RATE),
        ],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
        env=environment,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    assert completed.stdout == expected

    # And the digest is a claim about the values, not about object identity: a
    # second in-process draw canonicalises to the same bytes too.
    assert personas_digest(generate_personas(SEED, PERSONA_COUNT, HOUSEHOLD_RATE)) == expected


def test_no_clock_and_no_global_rng() -> None:
    """AC2: no clock and no module-level `random.*`, in the source and in behaviour."""
    source = Path(personas_module.__file__).read_text(encoding="utf-8")
    tree = ast.parse(source)

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                assert alias.name.split(".")[0] not in _CLOCK_MODULES, alias.name
        elif isinstance(node, ast.ImportFrom):
            assert (node.module or "").split(".")[0] not in _CLOCK_MODULES, node.module
        elif isinstance(node, ast.Attribute):
            assert node.attr not in _CLOCK_ATTRIBUTES, node.attr

    for statement in tree.body:
        if isinstance(statement, ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef):
            continue
        for node in ast.walk(statement):
            assert not (isinstance(node, ast.Call) and _is_random_call(node)), ast.dump(node)

    # The behavioural half: the process-global RNG is stirred between two draws and
    # neither the seeding nor the stirring reaches the output.
    random.seed(0)
    first = generate_personas(SEED, 256, HOUSEHOLD_RATE)
    random.seed(20260815)
    for _ in range(1000):
        random.random()
    random.shuffle(list(range(100)))
    second = generate_personas(SEED, 256, HOUSEHOLD_RATE)
    assert first == second


def test_seed_sensitivity(reference: list[Any]) -> None:
    """AC3: the seed moves the corpus; `n` is an argument and not a global."""
    other = generate_personas(SEED + 1, PERSONA_COUNT, HOUSEHOLD_RATE)
    changed = sum(
        1
        for mine, theirs in zip(reference, other, strict=True)
        if _identity(mine) != _identity(theirs)
    )
    assert changed >= 0.99 * PERSONA_COUNT

    larger = generate_personas(SEED, PERSONA_COUNT + 1, HOUSEHOLD_RATE)
    assert len(larger) == PERSONA_COUNT + 1

    again = generate_personas(SEED, PERSONA_COUNT, HOUSEHOLD_RATE)
    assert len(again) == PERSONA_COUNT
    assert again == reference


def test_name_frequency_skew_matches_weight_lists(reference: list[Any]) -> None:
    """AC4: the surnames are long-tailed, and their skew is the committed one."""
    counts = Counter(persona.family_name for persona in reference)
    ranked = counts.most_common()

    assert ranked[0][1] / PERSONA_COUNT >= 0.02
    assert sum(count for _, count in ranked[:10]) / PERSONA_COUNT >= 0.15

    weights = family_names()
    for name, weight in zip(weights.names, weights.weights, strict=True):
        expected = PERSONA_COUNT * weight / weights.total
        # The stated tolerance: four binomial standard deviations plus five, which at
        # this scale is generous for the lightest name (expected ~20) and still an
        # order of magnitude tighter than the gap between this list and a uniform
        # one. A tighter bound would fail on sampling noise for one of 120 names
        # roughly every other seed, which is a flaky test rather than a check.
        assert abs(counts[name] - expected) <= 4.0 * sqrt(expected) + 5.0, name


def test_household_rate_and_disjoint_identifiers(reference: list[Any]) -> None:
    """AC5: the realised share, and what a household may and may not share."""
    by_address: defaultdict[tuple[str | None, ...], list[Any]] = defaultdict(list)
    by_household: defaultdict[str, list[Any]] = defaultdict(list)
    for persona in reference:
        by_address[_address_key(persona)].append(persona)
        by_household[persona.household_id].append(persona)

    shared = sum(len(group) for group in by_address.values() if len(group) > 1)
    assert abs(shared / PERSONA_COUNT - HOUSEHOLD_RATE) <= RATE_TOLERANCE

    # An address group IS a household group: were two households ever to land on one
    # address, the corpus would carry a shared-address trap the ground truth does not
    # describe, and the rate above would be measuring something else.
    address_groups = sorted(
        tuple(sorted(persona.persona_id for persona in group)) for group in by_address.values()
    )
    household_groups = sorted(
        tuple(sorted(persona.persona_id for persona in group)) for group in by_household.values()
    )
    assert address_groups == household_groups

    multi = [group for group in by_household.values() if len(group) > 1]
    assert multi, "a household rate above zero must produce at least one household"
    for members in multi:
        assert len({_address_key(member) for member in members}) == 1
        for field in HOUSEHOLD_DISJOINT_FIELDS:
            values = [getattr(member, field) for member in members]
            assert len(set(values)) == len(values), field


def test_persona_ids_emails_and_phones_are_unique(reference: list[Any]) -> None:
    """AC6: unique ids of a fixed width, and no accidental cross-persona link."""
    pattern = re.compile(rf"{re.escape(PERSONA_ID_PREFIX)}[0-9]{{{PERSONA_ID_WIDTH}}}")

    identifiers = [persona.persona_id for persona in reference]
    assert len(set(identifiers)) == PERSONA_COUNT
    assert all(pattern.fullmatch(identifier) for identifier in identifiers)

    assert len({persona.email for persona in reference}) == PERSONA_COUNT
    assert len({persona.phone for persona in reference}) == PERSONA_COUNT

    # Fixed width means fixed: the id of persona 7 is the same string in a corpus of
    # twelve as in one of four thousand, so ground truth written at one scale cannot
    # be silently read against another.
    small = generate_personas(SEED, 12, HOUSEHOLD_RATE)
    assert [persona.persona_id for persona in small[:8]] == identifiers[:8]


def test_addresses_round_trip_through_regex_v1_parser(reference: list[Any]) -> None:
    """AC7: every `address_line` parses back into the components it was built from."""
    parser = RegexV1Parser()
    region_pattern = re.compile(r"[a-z]{2}")
    postal_pattern = re.compile(r"[0-9]{5}")

    for persona in reference:
        parsed = parser.parse(persona.address_line)
        assert (parsed.addr_number, parsed.addr_street, parsed.addr_unit) == (
            persona.addr_number,
            persona.addr_street,
            persona.addr_unit,
        ), persona.address_line
        # The other three are mapped from their own source columns rather than
        # parsed (S6), so what AC7 can hold them to is that they are already in the
        # spelling the mapping will pass through unchanged.
        assert persona.addr_city and persona.addr_city == persona.addr_city.strip().lower()
        assert region_pattern.fullmatch(persona.addr_region)
        assert postal_pattern.fullmatch(persona.addr_postal)


def test_birth_dates_are_full_precision(reference: list[Any]) -> None:
    """AC8: a full date, renderable under every configured source format."""
    assert set(DATE_FORMATS) == {"%Y-%m-%d", "%m/%d/%Y"}

    for persona in reference:
        assert isinstance(persona.birth_date, date)
        for date_format in DATE_FORMATS:
            rendered = persona.birth_date.strftime(date_format)
            assert datetime.strptime(rendered, date_format).date() == persona.birth_date

    # A year-only DOB would show up as every persona sharing one month and day; S4.2
    # NULLs such a value, so it would be ground truth the pipeline must discard.
    day_of_year = {(persona.birth_date.month, persona.birth_date.day) for persona in reference}
    assert len(day_of_year) > 300
