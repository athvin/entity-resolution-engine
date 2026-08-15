"""The S10.2 scale table, typed, plus the field accessor the S9.2 preflight shells out to.

The preflight runs `python benchmarks/scales.py --scale "$SCALE" --field <f>` inside
`er-pipeline:ci` with a bare `docker run` and pipes the result straight into
`$GITHUB_ENV`, so three properties of this module are contractual rather than stylistic:

* **One bare value on stdout, nothing else.** Anything else -- a log line, a banner, a
  trailing unit -- becomes part of `ER_CPU_LIMIT` and Compose then fails to parse a
  quota. Every diagnostic goes to stderr.
* **A non-zero exit on an unknown scale or field, with the offending value named.**
  The preflight has no way to tell "this scale has no envelope" from "this scale's
  envelope is the empty string", and the second silently measures a `100k` corpus in
  the 2-CPU default envelope -- a number that looks fine and means nothing (S9.2).
* **No import of `er`.** This runs in the image but not through the `er` entry point,
  before any lake exists, and it must not need a config, a catalog or a connection.

The consistency rules S10.2 states in prose are enforced in :func:`load_scales`, not
left to review: a `scales.yaml` that violates one is refused at load, so the failure
lands in the unit gate rather than four hours into a `1m` measurement.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Final

import yaml
from schema import BenchResultError

__all__ = [
    "FIELD_NAMES",
    "RUNNER_VCPUS",
    "SCALES_PATH",
    "Scale",
    "build_parser",
    "field_value",
    "get_scale",
    "load_scales",
    "main",
]

SCALES_PATH: Final[Path] = Path(__file__).resolve().parent / "scales.yaml"

#: vCPU count per GitHub runner label. `cpu_limit` may not exceed it: a
#: `deploy.resources.limits.cpus` above the machine's capacity is unenforceable, and
#: S10.4 would then be comparing every run against a limit none of them could reach.
#: Kept here rather than as an eleventh column because it is a property of the runner
#: image, not of the scale, and two scales already share `ubuntu-latest`.
RUNNER_VCPUS: Final[Mapping[str, int]] = MappingProxyType(
    {
        "ubuntu-latest": 2,
        "ubuntu-latest-8-cores": 8,
        "ubuntu-latest-16-cores": 16,
    }
)

#: Binary suffixes, because both readers of these strings are binary: Docker's
#: `memory:` and DuckDB's `SET memory_limit`. The absolute values matter to those two;
#: what matters *here* is only the ordering `duckdb_memory_limit < mem_limit`, and one
#: consistent base is what makes that comparison meaningful.
_MEMORY_UNITS: Final[Mapping[str, int]] = MappingProxyType(
    {
        "b": 1,
        "k": 1024,
        "kb": 1024,
        "kib": 1024,
        "m": 1024**2,
        "mb": 1024**2,
        "mib": 1024**2,
        "g": 1024**3,
        "gb": 1024**3,
        "gib": 1024**3,
    }
)

_SIZE_PATTERN: Final[re.Pattern[str]] = re.compile(r"^(?P<size>\d+)\s*(?P<unit>[A-Za-z]+)$")


@dataclass(frozen=True, slots=True)
class Scale:
    """One row of S10.2: what to generate, and what to measure it inside.

    The first four fields are the corpus and the preflight threshold; the last four
    are the resource envelope, and S10.2 makes them the single source of truth for the
    runner a scale is dispatched onto and the container limits it runs under.
    """

    name: str
    personas: int
    records: int
    incremental_batch: int
    min_free_gb: int
    baseline_committed: bool
    dispatchable: bool
    runner: str
    cpu_limit: int
    mem_limit: str
    duckdb_memory_limit: str


#: The ten S10.2 columns, in the order the spec's two tables state them. `--field`
#: accepts exactly these, so a typo in the workflow fails loudly instead of exporting
#: an empty variable.
FIELD_NAMES: Final[tuple[str, ...]] = (
    "personas",
    "records",
    "incremental_batch",
    "min_free_gb",
    "baseline_committed",
    "dispatchable",
    "runner",
    "cpu_limit",
    "mem_limit",
    "duckdb_memory_limit",
)

_INT_FIELDS: Final[frozenset[str]] = frozenset(
    {"personas", "records", "incremental_batch", "min_free_gb", "cpu_limit"}
)
_BOOL_FIELDS: Final[frozenset[str]] = frozenset({"baseline_committed", "dispatchable"})


def load_scales(path: Path | str = SCALES_PATH) -> Mapping[str, Scale]:
    """Read and validate `scales.yaml`.

    Returns:
        The scales in file order, keyed by name.

    Raises:
        BenchResultError: naming the offending scale, if a row is missing a column,
            holds the wrong type, or violates one of S10.2's three binding rules.
    """
    source = Path(path)
    document = yaml.safe_load(source.read_text(encoding="utf-8"))
    if not isinstance(document, dict):
        raise BenchResultError(f"{source}: expected a mapping of scale name to its row")

    scales: dict[str, Scale] = {}
    for name, row in document.items():
        scales[str(name)] = _build_scale(str(name), row, source)
    if not scales:
        raise BenchResultError(f"{source}: defines no scales")

    for scale in scales.values():
        _validate(scale)
    return MappingProxyType(scales)


def get_scale(name: str, path: Path | str = SCALES_PATH) -> Scale:
    """The row for `name`.

    Raises:
        BenchResultError: naming `name` and listing the scales that do exist.
    """
    scales = load_scales(path)
    if name not in scales:
        raise BenchResultError(f"unknown scale '{name}'; scales.yaml defines {sorted(scales)}")
    return scales[name]


def field_value(scale: Scale, field: str) -> str:
    """One S10.2 field, rendered for `$GITHUB_ENV`.

    Booleans render as `true`/`false` rather than Python's `True`/`False`: the shell
    reading them is comparing against the YAML spelling, and `True` is a value no
    `test`, `case` or workflow expression in this repo recognises.

    Raises:
        BenchResultError: naming `field`, if it is not one of the ten columns.
    """
    if field not in FIELD_NAMES:
        raise BenchResultError(
            f"unknown field '{field}'; scales.yaml columns are {list(FIELD_NAMES)}"
        )
    value = getattr(scale, field)
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


def build_parser() -> argparse.ArgumentParser:
    """The command line S9.2's preflight step calls."""
    parser = argparse.ArgumentParser(
        prog="python benchmarks/scales.py",
        description="Print one field of one benchmark scale (DesignDoc.md S10.2).",
    )
    parser.add_argument("--scale", required=True, help="scale name, e.g. smoke, 10k, 100k, 1m")
    parser.add_argument("--field", required=True, help=f"one of: {', '.join(FIELD_NAMES)}")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Print the requested field, or explain on stderr why it could not be.

    Returns:
        ``0`` on success, ``2`` on an unknown scale, an unknown field or an invalid
        `scales.yaml`. The codes are argparse's own and are local to this tool; they
        are not the S4.0 taxonomy, which no benchmark script participates in.
    """
    args = build_parser().parse_args(argv)
    try:
        value = field_value(get_scale(args.scale), args.field)
    except (BenchResultError, OSError) as exc:
        print(f"scales.py: {exc}", file=sys.stderr)
        return 2
    print(value)
    return 0


def _build_scale(name: str, row: Any, source: Path) -> Scale:
    """One row, type-checked column by column."""
    if not isinstance(row, dict):
        raise BenchResultError(f"{source}: scale '{name}' is not a mapping")

    missing = [field for field in FIELD_NAMES if field not in row]
    extra = [str(key) for key in row if key not in FIELD_NAMES]
    if missing or extra:
        raise BenchResultError(
            f"{source}: scale '{name}' must carry exactly the ten S10.2 columns "
            f"(missing {missing}, unexpected {extra})"
        )

    values: dict[str, Any] = {}
    for field in FIELD_NAMES:
        value = row[field]
        if field in _BOOL_FIELDS:
            if not isinstance(value, bool):
                raise BenchResultError(
                    f"{source}: scale '{name}' field '{field}' must be a boolean, got {value!r}"
                )
        elif field in _INT_FIELDS:
            # `isinstance(True, int)` is True, so booleans are rejected explicitly:
            # a `cpu_limit` of `yes` would otherwise become a quota of 1.
            if isinstance(value, bool) or not isinstance(value, int):
                raise BenchResultError(
                    f"{source}: scale '{name}' field '{field}' must be an integer, got {value!r}"
                )
        elif not isinstance(value, str):
            raise BenchResultError(
                f"{source}: scale '{name}' field '{field}' must be a string, got {value!r}"
            )
        values[field] = value
    return Scale(name=name, **values)


def _validate(scale: Scale) -> None:
    """S10.2's three binding rules, plus what the generator needs to be satisfiable."""
    if scale.personas < 1 or scale.records < scale.personas:
        raise BenchResultError(
            f"scale '{scale.name}': records ({scale.records}) must be at least personas "
            f"({scale.personas}); the generator cannot emit fewer rows than ground-truth persons"
        )

    vcpus = RUNNER_VCPUS.get(scale.runner)
    if vcpus is None:
        raise BenchResultError(
            f"scale '{scale.name}': runner '{scale.runner}' has no known vCPU count; "
            f"add it to RUNNER_VCPUS (known: {sorted(RUNNER_VCPUS)})"
        )
    if scale.cpu_limit > vcpus:
        raise BenchResultError(
            f"scale '{scale.name}': cpu_limit {scale.cpu_limit} exceeds the {vcpus} vCPU of "
            f"runner '{scale.runner}'; a quota above the machine's capacity is unenforceable "
            "and leaves S10.4 comparing against a limit no run could reach"
        )

    mem = _memory_bytes(scale.mem_limit, scale.name, "mem_limit")
    duckdb_mem = _memory_bytes(scale.duckdb_memory_limit, scale.name, "duckdb_memory_limit")
    if duckdb_mem >= mem:
        raise BenchResultError(
            f"scale '{scale.name}': duckdb_memory_limit {scale.duckdb_memory_limit} is not below "
            f"mem_limit {scale.mem_limit}; DuckDB's limit bounds the buffer manager only, so the "
            "Python heap and the dbt subprocess must fit in what is left"
        )

    if scale.dispatchable and not scale.baseline_committed:
        raise BenchResultError(
            f"scale '{scale.name}': dispatchable requires baseline_committed; a scale becomes "
            "dispatchable only once benchmarks/baselines/<scale>.json is committed (S9.2)"
        )


def _memory_bytes(text: str, scale_name: str, field: str) -> int:
    """`6g` / `4GB` as a byte count."""
    match = _SIZE_PATTERN.match(text.strip())
    if match is None or match.group("unit").lower() not in _MEMORY_UNITS:
        raise BenchResultError(
            f"scale '{scale_name}': {field} '{text}' is not a size like '6g' or '4GB'"
        )
    return int(match.group("size")) * _MEMORY_UNITS[match.group("unit").lower()]


if __name__ == "__main__":
    raise SystemExit(main())
