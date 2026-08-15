"""The scenario-directory layer of DesignDoc.md S8.2.1: shape, phases, manifest, loader.

``expected.py`` (ER-027) owns one expected *file*. This module owns the
*directory* a scenario is: the phase vocabulary, the eight header literals, the
``scenario.yaml`` manifest and its ``base_scenario`` composition, and
:func:`load_scenario`, which is how a scenario test opens a fixture.

Three rules are normative, and are why this is machinery rather than a
convention a fixture author is asked to remember:

* The phase vocabulary is exactly ``{base, batch, refresh, resurrect}`` and
  phases run in that order over the phases a scenario has; ``base`` is always
  present and always first. ``resurrect`` is not a spelling of ``refresh``:
  reusing ``refresh`` for it would tombstone every key the resurrection delivery
  omits, which is the opposite of what T-DEL-1 asserts.
* An ``expected/<phase>/<relation>.csv`` that does not exist means that phase
  makes **no claim** about that relation. It is not an empty expectation, so the
  loader reports ``None`` for it and never raises.
* ``base_scenario`` composes *inputs* only. A scenario declaring
  ``base_scenario: ok_minimal`` replays ``ok_minimal``'s deliveries before its
  own, but its expectations are its own: absence-is-no-claim is only readable
  when absence is local, and an inherited claim would be one no file in the
  scenario states.

The manifest is read by the strict parser below rather than by PyYAML because
``scripts/validate_fixtures.py`` is invoked as bare ``python3`` (S8.2.1's lint is
a repo script, and ``scripts/board.py`` and ``scripts/lint_spec.py`` set the
standing rule that those import the standard library only). The grammar it
accepts is pinned in ``fixtures/static/FORMAT.md`` and anything outside it is an
error naming the line, so the parser never guesses and cannot silently misread a
manifest a YAML library would read differently.
"""

from __future__ import annotations

import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Final

from .expected import LABEL_COLUMN, LABEL_PREFIX, ULID_SHAPED, Row, load_expected

__all__ = [
    "AUX_FILES",
    "BASE_PHASE",
    "EXPECTED_DIR",
    "EXPECTED_HEADERS",
    "EXPECTED_RELATIONS",
    "MANIFEST_NAME",
    "PHASES",
    "PHASE_PLACEHOLDER",
    "Manifest",
    "Scenario",
    "ScenarioError",
    "discover_scenarios",
    "expected_header_key",
    "load_scenario",
    "load_scenario_dir",
    "parse_manifest",
    "symbolic_label_error",
]

#: S8.2.1's phase vocabulary, in the order phases run. `base` is first because it
#: is the first delivery; the rest follow it in this order over the phases a
#: scenario declares.
PHASES: Final[tuple[str, ...]] = ("base", "batch", "refresh", "resurrect")

#: The phase every scenario has.
BASE_PHASE: Final[str] = PHASES[0]

#: The per-scenario manifest ER-028 adds to S8.2.1's shape: without it, "the
#: phases this scenario has" is inferred from whichever directories happen to be
#: on disk, and a delivery lost to a bad merge reads as a scenario that never had
#: that phase.
MANIFEST_NAME: Final[str] = "scenario.yaml"

#: The subdirectory each phase's expectations live under, inside the scenario
#: (S3: there is no top-level `fixtures/expected/`).
EXPECTED_DIR: Final[str] = "expected"

#: The three scenario-root files. They are inputs and bounds, not expectations,
#: which is why they sit beside `base/` rather than under `expected/`.
AUX_FILES: Final[tuple[str, ...]] = ("assertions.csv", "parity_pairs.csv", "tf_flip_pairs.csv")

#: The five relations a phase may make a claim about.
EXPECTED_RELATIONS: Final[tuple[str, ...]] = (
    "membership",
    "golden",
    "events",
    "std_hashes",
    "assertions",
)

#: What an `EXPECTED_HEADERS` key writes where a concrete phase name would go.
#: The keys are spelled exactly as S8.2.1 spells the paths, so the constant and
#: the spec compare character for character.
PHASE_PLACEHOLDER: Final[str] = "<phase>"

#: S8.2.1's eight literal header rows, as column tuples. Two files are named
#: `assertions.csv` and they are different relations -- the scenario-root INPUT
#: and a phase's expectation -- so the keys are paths, not file names.
EXPECTED_HEADERS: Final[Mapping[str, tuple[str, ...]]] = {
    "assertions.csv": ("phase", "rec_a_key", "rec_b_key", "kind", "created_by", "note"),
    "parity_pairs.csv": ("rec_a_key", "rec_b_key"),
    "tf_flip_pairs.csv": ("rec_a_key", "rec_b_key", "direction"),
    "expected/<phase>/membership.csv": (
        "persona_id",
        "source_system",
        "source_record_id",
        "entity_label",
    ),
    "expected/<phase>/golden.csv": (
        "entity_label",
        "given_name",
        "family_name",
        "email",
        "phone_e164",
        "addr_number",
        "addr_street",
        "addr_unit",
        "addr_city",
        "addr_region",
        "addr_postal",
        "birth_date",
        "survivorship_version",
    ),
    "expected/<phase>/events.csv": ("entity_label", "event_type", "count"),
    "expected/<phase>/std_hashes.csv": ("source_system", "source_record_id", "std_hash"),
    "expected/<phase>/assertions.csv": ("rec_a_key", "rec_b_key", "kind", "active"),
}

_REPO_ROOT: Final[Path] = Path(__file__).resolve().parents[2]

#: Where a scenario name is resolved, in order. `fixtures/static/` holds the real
#: scenarios (S8.2); `tests/fixtures/scenarios/` holds the linter's own valid and
#: negative self-test scenarios, which are test-owned reference material (S3) and
#: deliberately not shipped as fixtures.
SCENARIO_ROOTS: Final[tuple[Path, ...]] = (
    _REPO_ROOT / "fixtures" / "static",
    _REPO_ROOT / "tests" / "fixtures" / "scenarios",
)

#: The manifest's scalar alphabet. Scenario names, phase names and aux file names
#: are all identifiers or file names, so anything outside this set is an authoring
#: mistake rather than a value the strict parser should try to interpret.
_SCALAR = re.compile(r"[A-Za-z0-9_.-]+")

_MANIFEST_KEYS: Final[frozenset[str]] = frozenset(
    {"scenario", "phases", "base_scenario", "aux_files"}
)


class ScenarioError(ValueError):
    """A scenario that cannot be read as S8.2.1 describes one.

    Carries the file, the line and the lint rule the defect belongs to, so
    ``scripts/validate_fixtures.py`` can report it in its own format instead of
    re-deriving them from the message text.
    """

    def __init__(self, message: str, path: Path, line: int = 0, rule: str = "manifest") -> None:
        super().__init__(f"{path}:{line}: {rule}: {message}")
        self.detail = message
        self.path = path
        self.line = line
        self.rule = rule


def expected_header_key(relation: str) -> str:
    """The :data:`EXPECTED_HEADERS` key for one ``expected/<phase>/`` relation."""
    return f"{EXPECTED_DIR}/{PHASE_PLACEHOLDER}/{relation}.csv"


def symbolic_label_error(value: str | None) -> str | None:
    """Return why ``value`` is not an S8.2.1 symbolic label, or ``None`` if it is.

    ``expected.py`` states the same rule as an exception, which is right for a
    loader: the first bad label ends the read. A linter must report every one, so
    the predicate is spelled once here as a message-or-``None`` and
    :data:`~helpers.expected.ULID_SHAPED` is imported rather than re-written.
    """
    if not value:
        return f"{LABEL_COLUMN} is empty or NULL"
    if ULID_SHAPED.fullmatch(value):
        return (
            f"{LABEL_COLUMN} {value!r} is a ULID; expected files carry symbolic "
            f"{LABEL_PREFIX}1..{LABEL_PREFIX}n labels, never a minted entity_id"
        )
    return None


@dataclass(frozen=True)
class Manifest:
    """One parsed ``scenario.yaml``."""

    path: Path
    name: str
    phases: tuple[str, ...]
    base_scenario: str | None
    aux_files: tuple[str, ...]

    @property
    def directory(self) -> Path:
        return self.path.parent


def parse_manifest(path: Path) -> Manifest:
    """Parse and validate one ``scenario.yaml``.

    Raises:
        ScenarioError: the file is absent, a line is outside the pinned grammar,
            a required key is missing, or the declared phases are not a
            non-empty, duplicate-free subset of :data:`PHASES` containing
            :data:`BASE_PHASE`.
    """
    if not path.is_file():
        raise ScenarioError(f"no {MANIFEST_NAME}; every scenario directory carries one", path)

    fields: dict[str, str | tuple[str, ...]] = {}
    for line_number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        # No value in this grammar may contain `#`, so a comment is anything from
        # the first one to end of line.
        line = raw.split("#", 1)[0].rstrip()
        if not line:
            continue
        if line[0].isspace():
            raise ScenarioError(
                "indented line; the manifest is a flat 'key: value' map", path, line_number
            )
        key, separator, value = line.partition(":")
        if not separator:
            raise ScenarioError(f"{line!r} is not a 'key: value' line", path, line_number)
        key = key.strip()
        if key not in _MANIFEST_KEYS:
            raise ScenarioError(
                f"unknown key {key!r}; the manifest keys are {', '.join(sorted(_MANIFEST_KEYS))}",
                path,
                line_number,
            )
        if key in fields:
            raise ScenarioError(f"key {key!r} appears twice", path, line_number)
        fields[key] = _parse_value(value.strip(), path, line_number)

    return _manifest_from_fields(fields, path)


def _parse_value(value: str, path: Path, line_number: int) -> str | tuple[str, ...]:
    """Read one manifest value: a scalar, or a flow sequence ``[a, b]``."""
    if value.startswith("[") and value.endswith("]"):
        inner = value[1:-1].strip()
        items = [item.strip() for item in inner.split(",")] if inner else []
        for item in items:
            _require_scalar(item, path, line_number)
        return tuple(items)
    _require_scalar(value, path, line_number)
    return value


def _require_scalar(value: str, path: Path, line_number: int) -> None:
    if not _SCALAR.fullmatch(value):
        raise ScenarioError(
            f"{value!r} is not a bare scalar; manifest values are drawn from "
            "[A-Za-z0-9_.-] or are a flow sequence of them",
            path,
            line_number,
        )


def _manifest_from_fields(fields: Mapping[str, str | tuple[str, ...]], path: Path) -> Manifest:
    name = _required_scalar(fields, "scenario", path)
    if name != path.parent.name:
        raise ScenarioError(f"scenario is {name!r} but the directory is {path.parent.name!r}", path)

    phases = _sequence_field(fields, "phases", path)
    if phases is None:
        raise ScenarioError("no 'phases' key; every scenario declares its phases", path)
    if not phases:
        raise ScenarioError("'phases' is empty; every scenario has at least a base phase", path)
    unknown = [phase for phase in phases if phase not in PHASES]
    if unknown:
        raise ScenarioError(
            f"phase(s) {unknown} are outside the vocabulary {{{', '.join(PHASES)}}}", path
        )
    if len(set(phases)) != len(phases):
        raise ScenarioError(f"'phases' repeats a phase: {list(phases)}", path)
    if BASE_PHASE not in phases:
        raise ScenarioError(
            f"'phases' does not declare {BASE_PHASE!r}; it is always present and always first",
            path,
        )

    aux_files = _sequence_field(fields, "aux_files", path) or ()
    disallowed = [aux for aux in aux_files if aux not in AUX_FILES]
    if disallowed:
        raise ScenarioError(
            f"aux_files names {disallowed}; the scenario-root files are {list(AUX_FILES)}", path
        )

    return Manifest(
        path=path,
        name=name,
        # Declaration order is not the run order: S8.2.1 fixes the run order, so
        # the manifest is a set and this is where it becomes one.
        phases=tuple(phase for phase in PHASES if phase in set(phases)),
        base_scenario=_optional_scalar(fields, "base_scenario", path),
        aux_files=tuple(aux_files),
    )


def _required_scalar(fields: Mapping[str, str | tuple[str, ...]], key: str, path: Path) -> str:
    value = _optional_scalar(fields, key, path)
    if value is None:
        raise ScenarioError(f"no {key!r} key", path)
    return value


def _optional_scalar(
    fields: Mapping[str, str | tuple[str, ...]], key: str, path: Path
) -> str | None:
    value = fields.get(key)
    if value is None:
        return None
    if not isinstance(value, str):
        raise ScenarioError(f"{key!r} is a sequence; it takes a single name", path)
    return value


def _sequence_field(
    fields: Mapping[str, str | tuple[str, ...]], key: str, path: Path
) -> tuple[str, ...] | None:
    value = fields.get(key)
    if value is None:
        return None
    if isinstance(value, str):
        raise ScenarioError(f"{key!r} is a scalar; it takes a flow sequence [a, b]", path)
    return value


@dataclass(frozen=True)
class Scenario:
    """One S8.2.1 scenario directory, resolved.

    ``inputs`` and ``expected`` are keyed by phase, in :data:`PHASES` order
    restricted to the phases the manifest declares. ``expected[phase][relation]``
    is ``None`` when the file is absent, which is a phase making no claim about
    that relation rather than an empty expectation.
    """

    name: str
    root: Path
    manifest: Manifest
    phases: tuple[str, ...]
    base_chain: tuple[str, ...]
    inputs: Mapping[str, Mapping[str, Path]]
    expected: Mapping[str, Mapping[str, Path | None]]
    assertions: Mapping[str, tuple[Row, ...]]
    aux: Mapping[str, Path]

    def inputs_for(self, phase: str) -> Mapping[str, Path]:
        """The phase's per-source input CSVs, keyed by source name."""
        return self.inputs.get(phase, {})

    def expected_path(self, phase: str, relation: str) -> Path | None:
        """The phase's expectation for ``relation``, or ``None`` if it makes none."""
        return self.expected.get(phase, {}).get(relation)


def load_scenario(name: str, root: Path | None = None) -> Scenario:
    """Open the scenario called ``name``.

    ``root`` is searched before :data:`SCENARIO_ROOTS` when given, so a test can
    build a scenario in ``tmp_path`` without it being visible to the repo-wide
    lint.

    Raises:
        ScenarioError: no such scenario, a manifest outside the grammar, or a
            ``base_scenario`` cycle.
    """
    return load_scenario_dir(_resolve_directory(name, root))


def load_scenario_dir(directory: Path) -> Scenario:
    """Open the scenario at ``directory``. See :func:`load_scenario`."""
    chain = _manifest_chain(directory)
    manifest = chain[0]
    phases = manifest.phases
    return Scenario(
        name=manifest.name,
        root=directory,
        manifest=manifest,
        phases=phases,
        base_chain=tuple(ancestor.name for ancestor in chain[1:]),
        inputs={phase: _phase_inputs(chain, phase) for phase in phases},
        expected={phase: _phase_expected(directory, phase) for phase in phases},
        assertions=_assertions(directory),
        aux={name: directory / name for name in AUX_FILES if (directory / name).is_file()},
    )


def discover_scenarios(root: Path) -> tuple[Path, ...]:
    """Every scenario directory directly under ``root``, sorted.

    A scenario *is* a directory, so discovery does not look for a manifest: a
    directory that has lost its ``scenario.yaml`` must be discovered and reported,
    not skipped. Dot- and dunder-prefixed directories are tooling (``__pycache__``
    above all), never fixtures.
    """
    if not root.is_dir():
        return ()
    return tuple(
        sorted(
            entry
            for entry in root.iterdir()
            if entry.is_dir() and not entry.name.startswith((".", "__"))
        )
    )


def _resolve_directory(name: str, root: Path | None = None) -> Path:
    roots = (root, *SCENARIO_ROOTS) if root is not None else SCENARIO_ROOTS
    for candidate_root in roots:
        candidate = candidate_root / name
        if candidate.is_dir():
            return candidate
    searched = ", ".join(str(candidate_root) for candidate_root in roots)
    raise ScenarioError(f"no scenario named {name!r} under {searched}", roots[0] / name)


def _manifest_chain(directory: Path) -> list[Manifest]:
    """This scenario's manifest, then its ``base_scenario`` ancestors in order."""
    chain: list[Manifest] = []
    seen: list[str] = []
    current: Path | None = directory
    while current is not None:
        manifest = parse_manifest(current / MANIFEST_NAME)
        if manifest.name in seen:
            walked = " -> ".join([*seen, manifest.name])
            raise ScenarioError(
                f"base_scenario cycle {walked}: scenario {manifest.name!r} is its own ancestor "
                f"through scenario {seen[-1]!r}",
                manifest.path,
            )
        seen.append(manifest.name)
        chain.append(manifest)
        current = (
            None
            if manifest.base_scenario is None
            else _resolve_directory(manifest.base_scenario, current.parent)
        )
    return chain


def _phase_inputs(chain: list[Manifest], phase: str) -> Mapping[str, Path]:
    """The phase's per-source CSVs, taken from the nearest scenario that has them.

    A declared phase with no directory anywhere in the chain yields ``{}`` rather
    than an error: that is a lint failure with a rule and a file name of its own
    (``phase-dir``), and a loader that raised here would report it as a manifest
    defect instead.
    """
    for manifest in chain:
        directory = manifest.directory / phase
        if directory.is_dir():
            return {path.stem: path for path in sorted(directory.glob("*.csv"))}
    return {}


def _phase_expected(directory: Path, phase: str) -> Mapping[str, Path | None]:
    phase_dir = directory / EXPECTED_DIR / phase
    expected: dict[str, Path | None] = {}
    for relation in EXPECTED_RELATIONS:
        path = phase_dir / f"{relation}.csv"
        expected[relation] = path if path.is_file() else None
    return expected


def _assertions(directory: Path) -> Mapping[str, tuple[Row, ...]]:
    """The scenario-root ``assertions.csv``, grouped by its ``phase`` column."""
    path = directory / "assertions.csv"
    if not path.is_file():
        return {}
    try:
        rows = load_expected(path)
    except ValueError as exc:
        raise ScenarioError(str(exc), path, rule="header") from exc

    grouped: dict[str, list[Row]] = {}
    for line_number, row in enumerate(rows, start=2):
        phase = row.get("phase")
        if phase is None:
            raise ScenarioError(
                "assertion row has no phase; the phase column says when the "
                "assertion is applied and is never NULL",
                path,
                line_number,
                rule="assertion-phase",
            )
        grouped.setdefault(phase, []).append(row)
    return {phase: tuple(rows) for phase, rows in grouped.items()}
