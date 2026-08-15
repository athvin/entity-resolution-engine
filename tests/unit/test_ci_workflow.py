"""The PR workflow is asserted against DesignDoc.md S9.1, without GitHub.

Every property S9.1 makes load-bearing fails somewhere far from its cause, which is why
each is a test rather than a review note. A missing `permissions:` block grants the
default write token to every PR and nothing goes red. Serialised jobs cost wall-clock
that the <10 min budget has no room for, but produce identical results. A `uses:` that
slips back to a tag keeps working right up until the tag moves. And a `run:` step naming
a script no milestone has produced yet turns every PR red for three milestones -- the
failure mode S12 calls out by name, and the reason the M5-only steps are absent here.

Two sources, deliberately. The parsed document is what proves the semantics: what
`cache-to` actually resolves to, which job carries `needs:`, what the timeouts are. The
raw text is what proves the *authoring*: that each `uses:` carries the trailing tag
comment Dependabot rewrites, which parsing discards along with every other comment.
"""

from __future__ import annotations

import re
import subprocess
from functools import cache
from pathlib import Path
from typing import Any

import yaml

from er.versions import PINS

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yaml"
DEPENDABOT = REPO_ROOT / ".github" / "dependabot.yml"
ACTIONLINT_WRAPPER = REPO_ROOT / "scripts" / "ci" / "actionlint.sh"

# AC4, verbatim from the ticket: owner/repo at an unabbreviated 40-hex commit SHA.
SHA_PINNED = re.compile(r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$")
USES_LINE = re.compile(r"^\s*(?:-\s+)?uses:\s*(?P<ref>\S+)(?P<rest>.*)$")
TAG_COMMENT = re.compile(r"^\s*#\s*(?P<tag>v[0-9][\w.-]*)\s*$")

# S9.1's timeouts are the enforcement mechanism for the <10 min PR budget, not advice:
# `static` and `unit` run concurrently, so the wall clock is max(10, 10) + 25.
EXPECTED_TIMEOUTS = {"static": 10, "unit": 10, "integration": 25}

# AC6. The order matters twice over: the cheap lints have to fail before the expensive
# ones are paid for, and `uv sync --frozen` must precede everything that runs through it.
EXPECTED_STATIC_RUN_STEPS = [
    "uv sync --frozen",
    "uv run ruff check .",
    "uv run ruff format --check .",
    "uv run mypy --strict src/er",
    "bash scripts/ci/actionlint.sh",
    "uv run python scripts/lint_spec.py DesignDoc.md",
    "uv run python scripts/lint_board.py",
    "uv run dbt deps --project-dir dbt",
    "uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem",
]

EXPECTED_UNIT_RUN_STEPS = [
    "uv sync --frozen",
    "mkdir -p artifacts",
    "uv run dbt deps --project-dir dbt",
    "uv run dbt compile --project-dir dbt --profiles-dir dbt/profiles --target mem",
    "uv run pytest tests/unit -n auto -q --junitxml=artifacts/junit-unit.xml",
]

# AC5: the three directories whose contents a `run:` step can name. A path under one of
# them that does not exist is the M5-early-step failure mode, caught here rather than on
# the first pull request.
REPOSITORY_PATH = re.compile(r"\b(?:scripts|benchmarks|docker)/[\w./-]*[\w]")

# AC1: what the wrapper must not contain. Named here rather than in the script, so the
# script itself stays free of the tokens the assertion looks for.
FETCH_TOKENS = ("curl", "wget", "go install")

TEARDOWN = "down -v --remove-orphans"


@cache
def workflow_text() -> str:
    assert WORKFLOW.is_file(), f"{WORKFLOW.relative_to(REPO_ROOT)} does not exist"
    return WORKFLOW.read_text(encoding="utf-8")


@cache
def workflow_document() -> dict[Any, Any]:
    loaded = yaml.safe_load(workflow_text())
    assert isinstance(loaded, dict)
    return loaded


def job(name: str) -> dict[str, Any]:
    jobs = workflow_document()["jobs"]
    assert name in jobs, f"ci.yaml declares no job {name}"
    definition: dict[str, Any] = jobs[name]
    return definition


def run_steps(name: str) -> list[str]:
    """The `run:` scalars of one job, in order, with the block form joined.

    A block scalar is several shell commands in one step; every one of them is a
    repository path reference AC5 has to see, so they are flattened rather than kept as
    one opaque string.
    """
    commands: list[str] = []
    for step in job(name)["steps"]:
        if "run" not in step:
            continue
        commands.extend(line.strip() for line in step["run"].splitlines() if line.strip())
    return commands


def uses_lines() -> list[tuple[str, str]]:
    """Every `uses:` in the file as (reference, trailing text), read from the raw text.

    Read raw because the trailing tag comment is the half of the pin that parsing
    throws away, and it is the half Dependabot needs to propose the next SHA.
    """
    found: list[tuple[str, str]] = []
    for line in workflow_text().splitlines():
        match = USES_LINE.match(line)
        if match:
            found.append((match.group("ref"), match.group("rest")))
    assert found, "ci.yaml references no actions at all"
    return found


def upload_steps() -> list[dict[str, Any]]:
    steps = [
        step
        for definition in workflow_document()["jobs"].values()
        for step in definition["steps"]
        if str(step.get("uses", "")).startswith("actions/upload-artifact@")
    ]
    assert len(steps) == 2, "S9.1 uploads exactly two artifact sets: unit and integration"
    return steps


def test_permissions_concurrency_and_timeouts() -> None:
    document = workflow_document()

    # PyYAML resolves a bare `on` key to the YAML 1.1 boolean, so the trigger block is
    # found by value: asserting `document["on"]` would raise KeyError on a correct file.
    triggers = next(value for key, value in document.items() if key is True or key == "on")
    assert "pull_request" in triggers
    assert triggers["push"] == {"branches": ["main"]}

    # The default token is write-scoped. Read-only is the whole PR path needs, and an
    # absent block is indistinguishable from a passing run until something is published.
    assert document["permissions"] == {"contents": "read"}

    concurrency = document["concurrency"]
    assert concurrency["group"] == "${{ github.workflow }}-${{ github.ref }}"
    # Cancelling superseded runs on a branch is free; cancelling on `main` would abandon
    # the run that proves a merged commit, which is why the expression is not `true`.
    assert concurrency["cancel-in-progress"] == "${{ github.event_name == 'pull_request' }}"

    for name, minutes in EXPECTED_TIMEOUTS.items():
        assert job(name)["timeout-minutes"] == minutes, (
            f"{name} must be capped at {minutes} minutes; the S9.1 budget is enforced "
            "by these numbers and by nothing else"
        )


def test_job_graph_runs_static_and_unit_in_parallel() -> None:
    assert set(workflow_document()["jobs"]) == set(EXPECTED_TIMEOUTS)

    # The serial static -> unit -> integration chain paid for two `uv sync` runs ahead
    # of the expensive job and bought no extra signal (S9.1).
    for name in ("static", "unit"):
        assert "needs" not in job(name), (
            f"{name} must not depend on another job, or the two 10-minute jobs "
            "serialise into 20 minutes of wall clock for the same result"
        )
    assert job("integration")["needs"] == ["static", "unit"]


def test_all_uses_are_sha_pinned_with_tag_comments() -> None:
    for reference, rest in uses_lines():
        assert SHA_PINNED.match(reference), (
            f"{reference} is not pinned to a 40-character commit SHA; a tag is a "
            "moving target and a moving target is not a pin (S9)"
        )
        assert TAG_COMMENT.match(rest), (
            f"{reference} carries no trailing tag comment ({rest!r}); the comment is "
            "what Dependabot reads to work out which release comes next"
        )

    # S2.1's `uv` row: the action is pinned by SHA *and* told which uv to install, so
    # the runner resolves the lockfile with the version that produced it.
    for name in ("static", "unit"):
        setup = next(
            step
            for step in job(name)["steps"]
            if str(step.get("uses", "")).startswith("astral-sh/setup-uv@")
        )
        assert setup["with"]["version"] == PINS["uv"].version
        assert setup["with"]["enable-cache"] is True
        assert setup["with"]["cache-dependency-glob"] == "uv.lock"


def test_cache_to_is_a_quoted_scalar() -> None:
    build = next(
        step
        for step in job("integration")["steps"]
        if str(step.get("uses", "")).startswith("docker/build-push-action@")
    )
    # Both halves are the test. The parsed value is what buildx receives; the quoted
    # literal in the raw text is what keeps the comma inside the value if this mapping
    # is ever rewritten in flow style, where an unquoted comma ends the entry and the
    # cache silently degrades to inline-only.
    assert build["with"]["cache-to"] == "type=gha,mode=max"
    assert build["with"]["cache-from"] == "type=gha"
    assert 'cache-to: "type=gha,mode=max"' in workflow_text()


def test_run_steps_reference_existing_files() -> None:
    missing: list[str] = []
    for name in workflow_document()["jobs"]:
        for command in run_steps(name):
            for candidate in REPOSITORY_PATH.findall(command):
                if not (REPO_ROOT / candidate).exists():
                    missing.append(f"{name}: {command!r} names {candidate}")

    assert not missing, (
        "these steps name repository paths that do not exist:\n"
        + "\n".join(missing)
        + "\nA step referencing an artefact a later milestone produces leaves every PR "
        "red until that milestone lands, which is why the M5-only `lint_metrics.py` "
        "and `report.py --validate-baselines` steps are absent from this workflow (S12)."
    )


def test_static_job_step_order() -> None:
    assert run_steps("static") == EXPECTED_STATIC_RUN_STEPS
    assert run_steps("unit") == EXPECTED_UNIT_RUN_STEPS

    unit_uploads = [
        step
        for step in job("unit")["steps"]
        if str(step.get("uses", "")).startswith("actions/upload-artifact@")
    ]
    assert unit_uploads and unit_uploads[0]["with"]["name"] == "unit-artifacts"


def test_integration_job_teardown_and_uploads() -> None:
    steps = job("integration")["steps"]

    build = next(
        step for step in steps if str(step.get("uses", "")).startswith("docker/build-push-action@")
    )
    assert build["with"]["load"] is True, (
        "without `load: true` the image never reaches the local daemon, and compose's "
        "`pull_policy: never` then fails to find er-pipeline:ci (S7.1)"
    )
    assert build["with"]["tags"] == "er-pipeline:ci"
    assert build["with"]["file"] == "docker/Dockerfile"

    teardowns = [
        index for index, step in enumerate(steps) if "run" in step and TEARDOWN in step["run"]
    ]
    assert len(teardowns) == 2, "S9.1 tears the substrate down before the suite and after it"

    suite = next(
        index
        for index, step in enumerate(steps)
        if "run" in step and "pytest tests/integration" in step["run"]
    )
    # Before: a fresh runner's empty state is the state a local run has to reproduce.
    # After: `if: always()` or a failed suite leaks volumes into the next job.
    assert teardowns[0] < suite < teardowns[1]
    assert steps[teardowns[1]]["if"] == "always()"

    doctor = next(
        index
        for index, step in enumerate(steps)
        if "run" in step and step["run"].strip().endswith("er doctor")
    )
    assert teardowns[0] < doctor < suite, "T-DOCTOR-1 runs as the integration job's first check"

    for step in upload_steps():
        assert step["if"] == "always()", "a failed suite is exactly when its junit XML matters"
        assert step["with"]["if-no-files-found"] == "error", (
            "an empty artifacts directory means the bind mount never delivered; "
            "silence there would report a run that produced nothing as a success (M24)"
        )
        assert step["with"]["path"] == "artifacts/"


def test_doctor_is_first_integration_step() -> None:
    """AC7: the integration job's order is checkout/build/reset, `er doctor`, pytest.

    S9.1 and S2.1 both say `er doctor` runs *first* in this job, and S13 gives the
    reason: version skew and a broken substrate are what the doctor exists to catch,
    so the six T-DOCTOR-1 assertions have to fail before twenty minutes of suite time
    is spent discovering the same thing one test at a time. Asserted by step *index*
    rather than by presence, because a `er doctor` step that has drifted below the
    pytest step is still present and still passes -- and proves nothing.
    """
    steps = job("integration")["steps"]

    def index(predicate: Any) -> int:
        return next(position for position, step in enumerate(steps) if predicate(step))

    checkout = index(lambda step: str(step.get("uses", "")).startswith("actions/checkout@"))
    build = index(lambda step: str(step.get("uses", "")).startswith("docker/build-push-action@"))
    reset = index(lambda step: "run" in step and TEARDOWN in step["run"])
    doctor = index(lambda step: "run" in step and step["run"].strip().endswith("er doctor"))
    suite = index(lambda step: "run" in step and "pytest tests/integration" in step["run"])

    assert checkout < build < reset < doctor < suite, (
        "the integration job must check out, build the image, reset the substrate, "
        "run `er doctor` (T-DOCTOR-1) and only then run the suite; got indexes "
        f"checkout={checkout} build={build} reset={reset} doctor={doctor} suite={suite}"
    )
    # The doctor runs against the same Compose substrate the suite does, in the
    # `pipeline` service -- not on the runner, where there is no lake to assert.
    assert "--profile test run --rm pipeline er doctor" in steps[doctor]["run"]


def test_actionlint_wrapper_is_pinned_and_fetches_nothing() -> None:
    source = ACTIONLINT_WRAPPER.read_text(encoding="utf-8")
    for token in FETCH_TOKENS:
        assert token not in source, (
            f"scripts/ci/actionlint.sh contains {token!r}; a linter fetched at run time "
            "is an unpinned dependency policing a pinning rule (S2.1)"
        )

    result = subprocess.run(
        ["bash", str(ACTIONLINT_WRAPPER)],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
    )
    assert result.returncode == 0, (
        f"actionlint reported findings on the committed workflows:\n"
        f"{result.stdout}\n{result.stderr}"
    )
    assert PINS["actionlint-py"].version in result.stdout, (
        "the wrapper must name the actionlint version it ran, and that version must be "
        f"the S2.1 pin {PINS['actionlint-py'].version}; got: {result.stdout!r}"
    )


def test_actionlint_wrapper_rejects_an_unpinned_uses(tmp_path: Path) -> None:
    """The SHA-pin rule is enforced, not documented (AC2).

    actionlint 1.7.7 has no opinion about tags -- it exits 0 on `actions/checkout@v4` --
    so this arm is what distinguishes a workflow linter that happens to run from the
    policy S9 states. Without it, a floating tag reaches CI and nothing goes red.
    """
    reference, _ = uses_lines()[0]
    unpinned = f"{reference.split('@')[0]}@v4"
    workflows = tmp_path / "workflows"
    workflows.mkdir()
    (workflows / "ci.yaml").write_text(
        workflow_text().replace(reference, unpinned, 1), encoding="utf-8"
    )

    result = subprocess.run(
        ["bash", str(ACTIONLINT_WRAPPER), str(workflows)],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
    )
    assert result.returncode != 0, (
        f"the wrapper accepted the unpinned reference {unpinned}:\n{result.stdout}\n{result.stderr}"
    )
    assert unpinned in result.stderr, "the failure must name the reference that caused it"


def test_dependabot_tracks_the_github_actions_ecosystem() -> None:
    assert DEPENDABOT.is_file(), ".github/dependabot.yml does not exist"
    document = yaml.safe_load(DEPENDABOT.read_text(encoding="utf-8"))

    assert document["version"] == 2
    ecosystems = {update["package-ecosystem"] for update in document["updates"]}
    # Only this one. `uv.lock` is pinned to the literal versions of S2.1, so a bot
    # bumping a Python dependency would put the lockfile and the spec table out of
    # agreement -- which is a spec amendment, and which the spec lint fails on.
    assert ecosystems == {"github-actions"}

    actions = next(
        update for update in document["updates"] if update["package-ecosystem"] == "github-actions"
    )
    assert actions["directory"] == "/"
    assert actions["schedule"]["interval"] == "weekly"
