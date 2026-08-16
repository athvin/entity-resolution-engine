"""The "scenario tests never train" guard (S4.3.2 item 6, S12).

S12 says why this rule exists and why it needs a guard rather than a convention: EM
over `base_10`'s 23 records is degenerate — the u estimate is drawn from at most 253
pairs and the m estimate from at most 18 — so a scenario test that trained would score
against noise, and every quality number S8.3 asserts would be measuring that noise. It
would also pass. That is the failure mode a guard exists for: the wrong thing here is
green, slow and silent, and it is discovered only when a later ticket wonders why
`base_10`'s edge quality is unstable.

The guard is a source scan with an explicit allowlist, and both halves of that are
deliberate. A scan, because the offence is a property of the file rather than of a run
— a training call inside a fixture that a given selection never executes is still a
scenario that trains. An allowlist rather than a heuristic, because S12 names exactly
which tests exercise `er train`: T-TRAIN-1 and T-MODEL-1, and the `er train` lifecycle
suite they sit beside.

Comments are stripped before the scan. A comment naming a stage is documentation, not
an invocation — `tests/integration/test_concurrency.py` explains which commands take
the S4.0b writer lock and has to be able to say so — while a *string* is left in place
on purpose, because a command a test spawns reaches the guard as a string literal and
nothing else.
"""

from __future__ import annotations

import io
import tokenize
from dataclasses import dataclass
from pathlib import Path
from typing import Final

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

#: The scenario suite, which is what the rule is about (S8.3).
INTEGRATION_ROOT: Final = REPO_ROOT / "tests" / "integration"

#: The three files S12 permits to train, by base name. `test_model_lifecycle.py` is
#: T-MODEL-1's and is not written yet (ER-085); an allowlist entry for a file that
#: does not exist yet is how the guard stays true across the ticket that adds it,
#: rather than failing the moment it lands.
TRAINING_ALLOWLIST: Final[frozenset[str]] = frozenset(
    {"test_train.py", "test_train_reproducibility.py", "test_model_lifecycle.py"}
)

#: What "trains" looks like in a test: the CLI verb, the function that runs S4.3.2's
#: sequence, and the EM estimator itself — the last because a test that reached past
#: `er.matching.train` and drove Splink directly would be training just as much.
TRAINING_TOKENS: Final[tuple[str, ...]] = (
    "er train",
    "train_model",
    "estimate_parameters_using_expectation_maximisation",
)


@dataclass(frozen=True)
class TrainingReference:
    """One line that trains, located well enough to go and delete."""

    path: Path
    line: int
    token: str
    text: str

    def __str__(self) -> str:
        return f"{self.path}:{self.line}: {self.token}: {self.text.strip()}"


def strip_comments(source: str) -> str:
    """``source`` with every comment blanked out, line numbering preserved.

    Blanked rather than removed so a reported line number still points at the line the
    reader will open. A file that does not tokenize is returned unchanged: a syntax
    error is somebody else's failure, and swallowing the file here would make this
    guard the thing that hid it.
    """
    lines = source.splitlines()
    try:
        tokens = list(tokenize.generate_tokens(io.StringIO(source).readline))
    except (tokenize.TokenError, IndentationError, SyntaxError):
        return source
    for token in tokens:
        if token.exact_type != tokenize.COMMENT:
            continue
        row = token.start[0] - 1
        start, end = token.start[1], token.end[1]
        lines[row] = lines[row][:start] + " " * (end - start) + lines[row][end:]
    return "\n".join(lines)


def training_references(source: str, path: Path) -> tuple[TrainingReference, ...]:
    """Every line of ``source`` that trains, in file order, comments excluded."""
    found: list[TrainingReference] = []
    for number, line in enumerate(strip_comments(source).splitlines(), start=1):
        for token in TRAINING_TOKENS:
            if token in line:
                found.append(TrainingReference(path=path, line=number, token=token, text=line))
    return tuple(found)


def scan(
    root: Path, allowlist: frozenset[str] = TRAINING_ALLOWLIST
) -> tuple[TrainingReference, ...]:
    """Every training reference under ``root``, outside ``allowlist``, in path order."""
    found: list[TrainingReference] = []
    for path in sorted(root.rglob("*.py")):
        if path.name in allowlist:
            continue
        found.extend(training_references(path.read_text(encoding="utf-8"), path))
    return tuple(found)


def test_scenario_tests_never_train() -> None:
    """AC8: no file under `tests/integration/` outside the allowlist trains.

    The positive arm, run against the real suite. It is the assertion S4.3.2 item 6
    makes and the one S12 explains: the committed `fixtures/static/model_test_v1.json`
    is how a scenario acquires a model, and `tests/helpers/model.py` is how it loads it.
    """
    offences = scan(INTEGRATION_ROOT)
    assert not offences, (
        "these integration tests train, and scenario tests never train (S4.3.2 item 6, S12).\n"
        + "\n".join(str(offence) for offence in offences)
        + f"\nLoad the committed model through tests/helpers/model.py instead, or add the "
        f"file to TRAINING_ALLOWLIST if S12 says it exercises `er train`. "
        f"Allowlisted: {sorted(TRAINING_ALLOWLIST)}"
    )


def test_guard_detects_a_planted_training_call(tmp_path: Path) -> None:
    """AC8's negative arm: a synthetic offender is found, and a comment is not.

    Synthetic rather than committed for the obvious reason — a real offender in the
    tree would fail the positive arm above — and it covers all three tokens, because a
    guard that only recognised the CLI verb would miss a test that imported
    `er.matching.train` directly.
    """
    scenario = tmp_path / "test_planted_scenario.py"
    scenario.write_text(
        "\n".join(
            [
                "def test_scenario(connection):",
                '    run_shell("er train")  # a comment naming train_model must not count',
                "    result = train_model(connection, cfg, corpus, model_version='v0001')",
                "    linker.training.estimate_parameters_using_expectation_maximisation(rule)",
                "",
            ]
        ),
        encoding="utf-8",
    )
    documented = tmp_path / "test_documented_only.py"
    documented.write_text(
        "\n".join(
            [
                "# `er train` is a mutating command, so it takes the S4.0b writer lock.",
                "def test_lock_is_taken():",
                "    assert True",
                "",
            ]
        ),
        encoding="utf-8",
    )

    offences = scan(tmp_path)
    assert {offence.path for offence in offences} == {scenario}, (
        "the guard must fire on the planted file and stay silent on the one that only "
        f"names a stage in a comment; it reported {[str(o) for o in offences]}"
    )
    # All three tokens fire, one per line — and line 2 is reported once, for the
    # command it spawns, because the comment beside it naming `train_model` is blanked.
    assert {offence.token for offence in offences} == set(TRAINING_TOKENS)
    assert [offence.line for offence in offences] == [2, 3, 4], offences

    # And the allowlist is what makes the difference: the same file, allowlisted, is
    # not an offence.
    assert scan(tmp_path, frozenset({scenario.name})) == ()
