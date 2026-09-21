"""Check dependency pins and importable packages against the specification."""

from __future__ import annotations

import importlib
import re
import subprocess
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"
UV_LOCK = REPO_ROOT / "uv.lock"

# `uv` produces uv.lock; it is not a member of it. S2.1 asserts it through
# `er doctor: uv --version` and the Dockerfile tag instead, so it is the one
# pinned component the lockfile cross-check cannot see.
NOT_LOCKED = {"uv"}

# The distributions AC2 names explicitly. Parsing S2.1 is what actually drives the
# check; this set is the guard that a table reformat cannot quietly empty it.
REQUIRED_PINNED = {
    "splink",
    "duckdb",
    "dbt-core",
    "dbt-duckdb",
    "dbt-adapters",
    "dbt-common",
    "ruff",
    "mypy",
    "pytest",
    "pytest-xdist",
    "hypothesis",
    "actionlint-py",
    "typer",
    "pydantic",
    "python-ulid",
}

# These source directories are copied into the runtime image.
COPIED_DIRECTORIES = ("src", "configs", "benchmarks", "fixtures", "tests", "scripts")

PIN_RE = re.compile(r"`([A-Za-z0-9][A-Za-z0-9._-]*)(?:\[[^\]]*\])?==([A-Za-z0-9][A-Za-z0-9._-]*)`")


def normalize(name: str) -> str:
    """PEP 503 name normalisation; uv.lock stores names in this form."""
    return re.sub(r"[-_.]+", "-", name).lower()


def section(anchor: str) -> str:
    text = DESIGN_DOC.read_text(encoding="utf-8")
    start = text.find(f'<a id="{anchor}"></a>')
    assert start != -1, f"DesignDoc.md has no anchor {anchor}"
    end = text.find('<a id="', start + 1)
    return text[start:end] if end != -1 else text[start:]


def s2_1_pins() -> dict[str, str]:
    """Every `name==version` literal in the *Pin* column of the S2.1 table."""
    pins: dict[str, str] = {}
    for line in section("s2-1").splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if len(cells) < 4 or cells[0] in ("Component", "---") or set(cells[0]) <= {"-", ":"}:
            continue
        for name, version in PIN_RE.findall(cells[1]):
            if normalize(name) not in NOT_LOCKED:
                pins[normalize(name)] = version
    return pins


def locked_versions() -> dict[str, str]:
    with UV_LOCK.open("rb") as handle:
        lock = tomllib.load(handle)
    return {normalize(pkg["name"]): pkg["version"] for pkg in lock["package"]}


def git_lines(*args: str) -> list[str]:
    result = subprocess.run(
        ["git", *args], cwd=REPO_ROOT, capture_output=True, text=True, check=True
    )
    return [line for line in result.stdout.splitlines() if line]


def test_uv_lock_matches_s2_1_pins() -> None:
    pins = s2_1_pins()
    missing_from_table = REQUIRED_PINNED - set(pins)
    assert not missing_from_table, f"S2.1 no longer pins: {sorted(missing_from_table)}"

    locked = locked_versions()
    for distribution, version in sorted(pins.items()):
        assert distribution in locked, f"S2.1 pins {distribution} but uv.lock does not resolve it"
        assert locked[distribution] == version, (
            f"{distribution}: S2.1 says {version}, uv.lock resolves {locked[distribution]}"
        )


def s3_subpackages() -> set[str]:
    """Subdirectories of `src/er/` as drawn in the S3 layout tree."""
    lines = section("s3").splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith("├── src/er/"))
    names: set[str] = set()
    for line in lines[start + 1 :]:
        if line.startswith(("├── ", "└── ")):  # back out to the next top-level entry
            break
        match = re.match(r"^│   [├└]── ([a-z_]+)/", line)
        if match:
            names.add(match.group(1))
    assert names, "could not parse the src/er subpackages out of the S3 tree"
    return names


def test_every_src_subpackage_is_importable() -> None:
    expected = s3_subpackages()
    package_root = REPO_ROOT / "src" / "er"
    on_disk = {
        child.name
        for child in package_root.iterdir()
        if child.is_dir() and not child.name.startswith("__")
    }
    assert on_disk == expected, "src/er/ and the S3 tree disagree about the subpackages"
    assert (package_root / "py.typed").is_file(), "src/er/py.typed is missing"
    assert importlib.import_module("er").__version__

    for name in sorted(expected):
        assert (package_root / name / "__init__.py").is_file(), f"src/er/{name} has no __init__.py"
        importlib.import_module(f"er.{name}")


def test_s3_directories_exist_in_git() -> None:
    for directory in COPIED_DIRECTORIES:
        # --cached plus --others is the set a fresh clone gets once this change is
        # committed. Plain --cached would report nothing while the gates run,
        # because gates run against the working tree, before the commit exists.
        entries = git_lines(
            "ls-files", "--cached", "--others", "--exclude-standard", "--", directory
        )
        assert entries, f"{directory}/ has no file git would carry into a fresh clone"


def test_actionlint_wrapper_runs_the_pinned_binary_offline() -> None:
    # actionlint-py's version is the actionlint version plus a wheel build number,
    # so 1.7.7.23 ships actionlint 1.7.7.
    wheel_pin = s2_1_pins()["actionlint-py"]
    expected_version = ".".join(wheel_pin.split(".")[:3])

    result = subprocess.run(
        [sys.executable, "scripts/actionlint.py", "--version"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines()[0] == expected_version

    source = (REPO_ROOT / "scripts" / "actionlint.py").read_text(encoding="utf-8")
    for fetch in ("curl", "wget", "urlopen", "urllib", "requests."):
        assert fetch not in source, (
            f"scripts/actionlint.py references {fetch}; the binary must come from the pinned wheel"
        )
