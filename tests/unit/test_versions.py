"""Three-way parity for the S2.1 pin table: the spec, the module, the environment.

Nothing here is hand-copied from `src/er/versions.py`. Every expectation is parsed
out of the S2.1 table in DesignDoc.md or read out of the installed environment, so a
dependency bump that edits one of the three and not the others fails on the unit
layer rather than at `er doctor` inside an integration job.

The derivation from an S2.1 row to a component name is stated once — in
:func:`row_components` — and is the same rule the module documents:

* a row whose *Pin* cell contains ``name==version`` literals contributes one
  component per literal, named after the distribution with any extra stripped. That
  is the granularity ``importlib.metadata`` answers at, and S2.1's ``pytest`` and
  ``typer`` rows each pin more than one distribution;
* every other row contributes one component named after its *Component* cell: the
  last backticked token where there is one (``DuckDB extension `ducklake``` becomes
  ``ducklake``), else the cell with any parenthesised qualifier dropped;
* both are canonicalised the PEP 503 way — lowercased, with runs of ``-_.`` and
  whitespace collapsed to ``-``.

The rule is mechanical rather than a lookup table on purpose: a table mapping spec
rows to component names would itself need editing beside any new pin, which is the
edit these tests exist to force someone to make.
"""

from __future__ import annotations

import ast
import re
import tomllib
from importlib.metadata import version
from pathlib import Path

import duckdb
import splink

from er import __version__
from er.versions import (
    CODE_DISTRIBUTION,
    EXTENSION_PINS,
    IMAGE_PINS,
    PINS,
    SPLINK_MIGRATION_NOTE,
    Pin,
    check_installed_versions,
    code_version,
    installed_version,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
VERSIONS_SOURCE = REPO_ROOT / "src" / "er" / "versions.py"
DESIGN_DOC = (REPO_ROOT / "DesignDoc.md").read_text(encoding="utf-8")
PYPROJECT = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))

# A pinned requirement as S2.1 and pyproject.toml both write it, e.g.
# `psycopg[binary]==3.3.4`. The optional extra selects a wheel; it is not part of the
# distribution name `importlib.metadata` knows.
REQUIREMENT = re.compile(r"([A-Za-z0-9_.-]+?)(?:\[[a-z0-9,-]+\])?==([^`\s,]+)")

# The three service images are pinned by digest, never by a mutable tag (S2.1).
IMAGE_REFERENCE = re.compile(r"(\S+):([^@\s]+)@(sha256:[0-9a-f]{64})")

# The only names `src/er/versions.py` may reach for. Anything that opens a connection
# or shells out belongs to `er doctor` (ER-022), and :data:`FORBIDDEN_CALLS` below is
# what now enforces that half — the list has to admit the three modules S3's second
# assignment to this file needs ("version-compat guards for run-all", ER-034): the
# guard's `last_successful_run` is HANDED a connection and reads `runs` through it, so
# it needs `duckdb` for the annotation, the lake alias and the schema qualifier. All
# three are import-safe: none of them opens anything at import time, so the pin table
# stays readable without an engine.
ALLOWED_IMPORTS = frozenset(
    {
        "__future__",
        "collections.abc",
        "dataclasses",
        "duckdb",
        "enum",
        "er",
        "er.lake.ducklake",
        "er.lake.model",
        "importlib.metadata",
        "typing",
    }
)

# What "pure" actually protects, now that the module is allowed to name the lake: it
# may READ through a connection it was given and it may never OPEN one or shell out,
# because `er doctor` imports this module before it knows whether it can reach the lake
# at all and ER-007's Dockerfile check imports it in a build stage that has neither a
# catalog nor an object store. Matched on the last segment of a called name, so
# `duckdb.connect`, `ducklake.connect` and a bare `connect` are one rule.
FORBIDDEN_CALL_NAMES = frozenset(
    {"connect", "catalog_connect", "run", "Popen", "check_output", "check_call", "urlopen"}
)


def canonical(name: str) -> str:
    """PEP 503 canonical form, which is also this table's component-name form."""
    return re.sub(r"[-_.\s]+", "-", name.strip().strip("*`").lower()).strip("-")


def s2_1_table() -> list[tuple[str, str, str]]:
    """The (component, pin, asserted-by) cells of every S2.1 row, in spec order."""
    section = DESIGN_DOC.split('<a id="s2-1"></a>')[1].split('<a id="')[0]
    rows = []
    for line in section.splitlines():
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 4 or cells[0] == "Component" or set(cells[0]) <= {"-", ":"}:
            continue
        rows.append((cells[0], cells[1], cells[3]))
    assert len(rows) >= 20, f"parsed only {len(rows)} rows out of the S2.1 table"
    return rows


def row_components(component_cell: str, pin_cell: str) -> dict[str, str]:
    """Component name → pinned literal for one S2.1 row (the module docstring's rule)."""
    requirements = REQUIREMENT.findall(pin_cell)
    if requirements:
        return {canonical(name): pinned for name, pinned in requirements}
    ticked = re.findall(r"`([^`]+)`", component_cell)
    name = ticked[-1] if ticked else re.sub(r"\([^)]*\)", "", component_cell)
    literal = re.search(r"`([^`]+)`", pin_cell)
    assert literal is not None, f"the {component_cell} row states no pinned literal"
    return {canonical(name): literal.group(1)}


def spec_components() -> dict[str, str]:
    """Component name → pinned literal, over the whole S2.1 table."""
    components: dict[str, str] = {}
    for component_cell, pin_cell, _ in s2_1_table():
        components.update(row_components(component_cell, pin_cell))
    return components


def spec_doctor_components() -> set[str]:
    """The components whose S2.1 *Asserted by* cell names `er doctor`."""
    return {
        component
        for component_cell, pin_cell, asserted_by in s2_1_table()
        if "er doctor" in asserted_by
        for component in row_components(component_cell, pin_cell)
    }


def declared_distributions() -> set[str]:
    """Every distribution pyproject.toml declares, runtime and dev group alike."""
    declared = list(PYPROJECT["project"]["dependencies"])
    for group in PYPROJECT["dependency-groups"].values():
        declared.extend(group)
    names = set()
    for requirement in declared:
        matched = REQUIREMENT.fullmatch(requirement)
        assert matched is not None, f"pyproject declares an unpinned requirement: {requirement}"
        names.add(canonical(matched.group(1)))
    return names


def spec_row(prefix: str) -> str:
    """The one DesignDoc.md table row whose first cell starts with ``prefix``."""
    rows = [line for line in DESIGN_DOC.splitlines() if line.startswith(f"| {prefix}")]
    assert len(rows) == 1, f"expected exactly one `{prefix}` row, found {len(rows)}"
    return rows[0]


def test_pins_cover_exactly_the_s2_1_doctor_rows() -> None:
    expected = spec_components()

    # Both directions, named separately: a spec row with no `PINS` entry and a `PINS`
    # entry with no spec row are different mistakes and must read differently.
    assert set(PINS) - set(expected) == set(), "PINS carries components S2.1 does not"
    assert set(expected) - set(PINS) == set(), "S2.1 pins components PINS does not carry"

    for component, pinned in expected.items():
        assert PINS[component].version == pinned, component

    # The doctor's own subset. S2.1's closing rule — "a component with no row is not
    # asserted and therefore is not pinned" — is only true if this holds exactly.
    doctor = {component for component, pin in PINS.items() if pin.asserted_by_doctor}
    assert doctor == spec_doctor_components()

    # The rows the lockfile and CI assert but `er doctor` does not, spelled out:
    # flipping one of them to `er doctor` in S2.1 without adding the runtime check —
    # or the reverse — must not pass quietly.
    assert set(PINS) - doctor == {
        "dbt-adapters",
        "dbt-common",
        "hypothesis",
        "object-store-init-image",
    }


def test_installed_versions_match_pins() -> None:
    # Every pin that names a distribution agrees with the synced environment.
    for pin in PINS.values():
        if pin.distribution is None:
            continue
        assert installed_version(pin.distribution) == pin.version, pin.component

    # The two libraries S2.1 asserts through a module attribute rather than through
    # metadata agree with it too, because the attribute is what `er doctor` reads.
    assert splink.__version__ == PINS["splink"].version
    assert duckdb.__version__ == PINS["duckdb"].version

    # S2.1's rule that adding a dependency means adding a row: the distribution-backed
    # pins are exactly pyproject's declared set, in both directions.
    backed = {pin.distribution for pin in PINS.values() if pin.distribution is not None}
    assert backed == declared_distributions()

    # A pin with no distribution is one nothing in the metadata database can answer
    # for: the interpreter, the three extensions, the three images, and `uv`, which
    # produced the lockfile but is not installed into this environment.
    unbacked = {component for component, pin in PINS.items() if pin.distribution is None}
    assert unbacked == {
        "python",
        "ducklake",
        "postgres",
        "httpfs",
        "uv",
        "catalog-image",
        "object-store-image",
        "object-store-init-image",
    }


def test_extension_pins_and_registered_names() -> None:
    assert {name: pin.version for name, pin in EXTENSION_PINS.items()} == {
        "ducklake": "d8a1881e",
        "postgres": "41223e5",
        "httpfs": "827222f",
    }

    # The distinction M25 exists for: `INSTALL postgres`, but `duckdb_extensions()`
    # reports `postgres_scanner`. A check filtered on the install name matches no row
    # and passes while asserting nothing.
    assert EXTENSION_PINS["postgres"].registered_name == "postgres_scanner"
    assert "postgres_scanner" in spec_row("DuckDB extension `postgres`")
    for name in ("ducklake", "httpfs"):
        assert EXTENSION_PINS[name].registered_name == name

    # Each commit hash equals its S2.1 cell, and each key is the install name.
    spec = spec_components()
    for name, extension in EXTENSION_PINS.items():
        assert extension.version == spec[name], name
        assert extension.install_name == name


def test_image_digests_match_s2_1() -> None:
    spec = spec_components()
    references = {
        "catalog": spec["catalog-image"],
        "objectstore": spec["object-store-image"],
        "objectstore-init": spec["object-store-init-image"],
    }
    assert set(IMAGE_PINS) == set(references)

    for service, reference in references.items():
        image = IMAGE_PINS[service]
        # Byte equality with the S2.1 cell, not "the digest appears somewhere in it".
        assert image.reference == reference, service
        assert image.service == service

        matched = IMAGE_REFERENCE.fullmatch(reference)
        assert matched is not None, f"S2.1 pins {service} by a mutable tag: {reference}"
        assert (image.repository, image.tag, image.digest) == matched.groups()
        assert len(image.digest.removeprefix("sha256:")) == 64


def test_check_installed_versions_reports_mismatch() -> None:
    checks = check_installed_versions()

    # One row per distribution-backed pin, and every row green in a synced tree.
    assert {check.component for check in checks} == {
        pin.component for pin in PINS.values() if pin.distribution is not None
    }
    assert [check for check in checks if not check.ok] == []
    for check in checks:
        assert check.actual == check.expected

    # A fabricated expectation map reports, it does not raise: `er doctor` prints
    # every check before exiting 1, so one bad row must not hide the twenty after it.
    fabricated = {
        "splink": Pin("splink", "9.9.9", "splink", asserted_by_doctor=True),
        "absent": Pin("absent", "1.0.0", "er-does-not-ship-this", asserted_by_doctor=True),
        "python": Pin("python", "3.12", None, asserted_by_doctor=True),
    }
    fabricated_checks = check_installed_versions(fabricated)

    # The pin with no distribution contributes no row; the other two report False.
    assert [check.component for check in fabricated_checks] == ["splink", "absent"]
    assert [check.ok for check in fabricated_checks] == [False, False]
    assert fabricated_checks[0].actual == PINS["splink"].version
    assert fabricated_checks[1].actual is None


def test_code_version_is_the_installed_er_distribution() -> None:
    assert CODE_DISTRIBUTION == "er"
    assert code_version() == version("er")
    assert code_version()
    # The metadata and the package attribute are two spellings of one fact, and
    # `code_version()` falls back from the first to the second, so they must agree.
    assert code_version() == __version__


def test_splink_major_is_4_and_migration_note_is_actionable() -> None:
    note = SPLINK_MIGRATION_NOTE
    assert note.distribution == "splink"

    # The guard: Splink 5 removes the primitive the incremental new-vs-corpus pass is
    # built on, so a lockfile refresh that bumps the major must fail here.
    assert int(PINS["splink"].version.split(".")[0]) == note.pinned_major == 4
    assert note.breaking_major == 5

    assert "find_matches_to_new_records" in note.removed
    assert note.replacement == ("predict_between", "predict_within")
    assert note.blast_radius == ("src/er/matching/incremental.py",)
    assert set(note.acceptance_tests) == {"T-INC-3", "T-BLK-1"}

    # Every name the note carries is a name S13 states; a note that had drifted from
    # its spec row would point the migration at the wrong API.
    s13_row = spec_row("**Splink 5 migration.**")
    for named in (*note.removed, *note.replacement, *note.blast_radius, *note.acceptance_tests):
        assert named in s13_row, named


def test_every_public_constant_is_final_annotated() -> None:
    # AC8, structurally. `mypy --strict` accepts an unannotated module constant, but
    # an unannotated one is rebindable, and `er doctor`, the Dockerfile extension
    # check and the compose contract test all read these tables as immutable.
    tree = ast.parse(VERSIONS_SOURCE.read_text(encoding="utf-8"))
    annotated: set[str] = set()
    for node in tree.body:
        if isinstance(node, ast.Assign):
            names = [t.id for t in node.targets if isinstance(t, ast.Name)]
            assert names == ["__all__"], f"{names} is assigned without a Final annotation"
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            annotation = ast.unparse(node.annotation)
            assert annotation.startswith("Final["), f"{node.target.id}: {annotation}"
            annotated.add(node.target.id)

    assert {"PINS", "EXTENSION_PINS", "IMAGE_PINS", "SPLINK_MIGRATION_NOTE"} <= annotated


def test_module_is_pure() -> None:
    # The pin table must be readable without an engine: `er doctor` imports it before
    # it knows whether it can reach the lake at all, and ER-007's Dockerfile check
    # imports it inside a build stage that has no catalog and no object store.
    tree = ast.parse(VERSIONS_SOURCE.read_text(encoding="utf-8"))
    imported: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imported |= {alias.name for alias in node.names}
        elif isinstance(node, ast.ImportFrom):
            assert node.level == 0, "versions.py uses a relative import"
            assert node.module is not None
            imported.add(node.module)

    assert imported <= ALLOWED_IMPORTS, f"versions.py imports {sorted(imported - ALLOWED_IMPORTS)}"

    # The half the allowlist can no longer carry on its own. `er.lake.ducklake` is
    # importable here precisely because importing it opens nothing; calling its
    # `connect()` would open everything, and this module must never be the caller.
    called = {ast.unparse(node.func) for node in ast.walk(tree) if isinstance(node, ast.Call)}
    opened = {name for name in called if name.rsplit(".", 1)[-1] in FORBIDDEN_CALL_NAMES}
    assert not opened, f"versions.py calls {sorted(opened)}; opening one belongs to its caller"
