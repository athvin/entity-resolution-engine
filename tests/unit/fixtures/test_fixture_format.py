"""Unit tests for the S8.2.1 fixture format: the vocabulary, the loader, the linter.

Every scenario in S8.2 is one directory with the same shape, and none of them can
be authored until that shape is mechanical. Three properties carry the weight
here, and each is asserted against a committed artefact rather than against the
code's own idea of itself:

* The eight header rows are transcribed from S8.2.1 into this file and compared
  to `EXPECTED_HEADERS` character for character, and `FORMAT.md`'s quoted block
  is compared to the same constant. A header can then drift only by drifting in
  three places at once.
* A phase directory that is absent means the scenario has no such phase, and an
  `expected/<phase>/` file that is absent means that phase makes **no claim**
  about that relation. The second is the one a loader gets wrong by raising, so
  it has a test of its own and a committed scenario with a deliberately missing
  `golden.csv`.
* The linter is parametrised over its **own** rule list (`--list-rules`), not
  over a list written here, and each rule's negative fixture is required to fail
  for exactly that rule and no other. A rule added without a fixture fails
  collection; a fixture that fails for a second reason fails its own test. A
  linter with no failing arm proves nothing.

The negative fixtures carry two-column input CSVs. The linter does not read input
headers -- those are S6's `sources.<name>.columns` mapping and V11's business,
not this format's -- so a fixture whose only job is to be otherwise well-formed
carries the least content that makes it so.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

import pytest
from helpers.scenario import (
    AUX_FILES,
    BASE_PHASE,
    EXPECTED_HEADERS,
    MANIFEST_NAME,
    PHASES,
    ScenarioError,
    load_scenario,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
LINTER = REPO_ROOT / "scripts" / "validate_fixtures.py"
FORMAT_MD = REPO_ROOT / "fixtures" / "static" / "FORMAT.md"
SELFTEST_ROOT = REPO_ROOT / "tests" / "fixtures" / "scenarios"

# S8.2.1's eight literal header rows, transcribed. This is the copy the constant
# is graded against, so it is written out rather than derived from anything.
S8_2_1_HEADER_ROWS = {
    "assertions.csv": "phase,rec_a_key,rec_b_key,kind,created_by,note",
    "parity_pairs.csv": "rec_a_key,rec_b_key",
    "tf_flip_pairs.csv": "rec_a_key,rec_b_key,direction",
    "expected/<phase>/membership.csv": "persona_id,source_system,source_record_id,entity_label",
    "expected/<phase>/golden.csv": (
        "entity_label,given_name,family_name,email,phone_e164,addr_number,addr_street,"
        "addr_unit,addr_city,addr_region,addr_postal,birth_date,survivorship_version"
    ),
    "expected/<phase>/events.csv": "entity_label,event_type,count",
    "expected/<phase>/std_hashes.csv": "source_system,source_record_id,std_hash",
    "expected/<phase>/assertions.csv": "rec_a_key,rec_b_key,kind,active",
}

#: Which committed negative fixture is the failing arm of which rule.
NEGATIVE_FIXTURES = {
    "manifest": "bad_manifest",
    "phase-dir": "bad_phase_dir",
    "expected-phase": "bad_expected_phase",
    "unknown-file": "bad_root_file",
    "header": "bad_header",
    "volatile-column": "bad_volatile_column",
    "entity-label": "bad_entity_label",
    "null-token": "bad_null_token",
    "sort-order": "bad_sort_order",
    "assertion-phase": "bad_assertion_phase",
}

#: `path:line: rule: message`, the linter's one output shape.
VIOLATION = re.compile(r"^\S+:\d+: (?P<rule>[a-z-]+): ")


def _run_linter(*arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(LINTER), *arguments],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
        check=False,
    )


def _linter_rules() -> tuple[str, ...]:
    """The linter's own rule vocabulary, so the coverage below cannot be stale."""
    result = _run_linter("--list-rules")
    if result.returncode != 0 or not result.stdout.strip():
        raise RuntimeError(f"{LINTER} --list-rules failed: {result.returncode} {result.stderr}")
    return tuple(result.stdout.split())


def _rules_reported(stderr: str) -> set[str]:
    return {
        match.group("rule")
        for match in (VIOLATION.match(line) for line in stderr.splitlines())
        if match is not None
    }


def _format_md_headers() -> dict[str, str]:
    """The header rows quoted in `FORMAT.md`, as `path -> comma-joined columns`."""
    block = re.search(
        r"## Headers \(literal\).*?```text\n(?P<body>.*?)```", FORMAT_MD.read_text("utf-8"), re.S
    )
    assert block is not None, "FORMAT.md has no fenced '## Headers (literal)' block"

    headers: dict[str, str] = {}
    pending: str | None = None
    for line in block.group("body").splitlines():
        stripped = line.strip()
        if not stripped:
            pending = None
            continue
        # The name line carries a trailing `# ...` note; the header row never does.
        value = stripped.split("#", 1)[0].strip()
        if pending is None:
            pending = value
        else:
            headers[pending] = value
            pending = None
    return headers


def _write_manifest(root: Path, name: str, body: str, phase_dirs: tuple[str, ...]) -> Path:
    directory = root / name
    for phase in phase_dirs:
        (directory / phase).mkdir(parents=True, exist_ok=True)
        (directory / phase / "crm.csv").write_text("crm_id,persona_id\nc1,P1\n", encoding="utf-8")
    directory.mkdir(parents=True, exist_ok=True)
    (directory / MANIFEST_NAME).write_text(body, encoding="utf-8")
    return directory


def test_expected_headers_match_s8_2_1_literals() -> None:
    assert set(EXPECTED_HEADERS) == set(S8_2_1_HEADER_ROWS)
    assert len(EXPECTED_HEADERS) == 8
    for key, row in S8_2_1_HEADER_ROWS.items():
        assert ",".join(EXPECTED_HEADERS[key]) == row, key
    # The two `assertions.csv` are different relations, which is why the keys are
    # paths: the scenario-root INPUT leads with `phase`, the expectation does not.
    assert EXPECTED_HEADERS["assertions.csv"][0] == "phase"
    assert "phase" not in EXPECTED_HEADERS["expected/<phase>/assertions.csv"]


def test_format_md_headers_match_constants() -> None:
    assert _format_md_headers() == {
        key: ",".join(columns) for key, columns in EXPECTED_HEADERS.items()
    }

    # The encoding rules are quoted in the same document and are just as
    # load-bearing, so they are held to the same standard as the headers.
    text = FORMAT_MD.read_text("utf-8")
    for literal in ("`\\N`", "`1e-9`", "`VOLATILE_COLUMNS`", "`E1, E2, … En`"):
        assert literal in text, literal
    for aux in AUX_FILES:
        assert aux in text, aux


def test_phase_vocabulary_and_ordering(tmp_path: Path) -> None:
    assert PHASES == ("base", "batch", "refresh", "resurrect")
    assert BASE_PHASE == "base"
    assert "`{" + ", ".join(PHASES) + "}`" in FORMAT_MD.read_text("utf-8")

    # Declaration order is not run order: `composed` declares [batch, base].
    assert load_scenario("composed").phases == ("base", "batch")
    assert load_scenario("ok_minimal").phases == ("base",)

    _write_manifest(tmp_path, "no_base", "scenario: no_base\nphases: [batch]\n", ("batch",))
    with pytest.raises(ScenarioError, match="always present and always first"):
        load_scenario("no_base", tmp_path)

    _write_manifest(tmp_path, "off_vocab", "scenario: off_vocab\nphases: [base, staging]\n", ())
    with pytest.raises(ScenarioError, match="outside the vocabulary"):
        load_scenario("off_vocab", tmp_path)


def test_load_scenario_returns_phases_inputs_and_expectations() -> None:
    scenario = load_scenario("ok_minimal")

    assert scenario.phases == ("base",)
    assert scenario.base_chain == ()
    assert set(scenario.inputs["base"]) == {"billing", "crm"}
    assert scenario.inputs["base"]["crm"].name == "crm.csv"
    assert scenario.inputs_for("base") == scenario.inputs["base"]

    membership = scenario.expected_path("base", "membership")
    assert membership is not None
    assert membership.read_text("utf-8").splitlines()[0] == ",".join(
        EXPECTED_HEADERS["expected/<phase>/membership.csv"]
    )

    assert set(scenario.assertions) == {"base"}
    (assertion,) = scenario.assertions["base"]
    assert assertion["rec_a_key"] == "billing:b1"
    assert assertion["kind"] == "always"
    assert set(scenario.aux) == {"assertions.csv"}


def test_absent_expected_file_is_no_claim() -> None:
    scenario = load_scenario("ok_minimal")

    assert not (scenario.root / "expected" / "base" / "golden.csv").exists()
    assert scenario.expected["base"]["golden"] is None
    assert scenario.expected_path("base", "golden") is None
    # An expectation absent for a relation is not an absent phase: the phase is
    # still there and still claims the three relations it does commit.
    assert scenario.expected["base"]["membership"] is not None
    assert scenario.expected["base"]["assertions"] is None


def test_base_scenario_composition_and_cycle_detection() -> None:
    composed = load_scenario("composed")
    ok_minimal = load_scenario("ok_minimal")

    assert composed.base_chain == ("ok_minimal",)
    assert composed.inputs["base"] == ok_minimal.inputs["base"]
    assert set(composed.inputs["batch"]) == {"crm"}
    assert composed.inputs["batch"]["crm"].parent.parent == composed.root

    # Inputs compose; claims do not. `composed` inherits ok_minimal's base
    # delivery but states no expectation about it.
    assert composed.expected["base"]["membership"] is None
    assert composed.expected["batch"]["membership"] is not None

    with pytest.raises(ScenarioError) as excinfo:
        load_scenario("cycle_a")
    message = str(excinfo.value)
    assert "cycle_a" in message and "cycle_b" in message


@pytest.mark.parametrize("rule", _linter_rules())
def test_each_negative_fixture_fails_for_its_own_rule(rule: str) -> None:
    # A KeyError here is the point: a rule with no committed failing arm.
    fixture = SELFTEST_ROOT / NEGATIVE_FIXTURES[rule]
    result = _run_linter(str(fixture))

    assert result.returncode == 1, f"{fixture} was accepted:\n{result.stdout}{result.stderr}"
    assert _rules_reported(result.stderr) == {rule}, (
        f"{fixture} must fail for {rule!r} alone:\n{result.stderr}"
    )
