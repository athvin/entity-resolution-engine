"""Validation of the benchmark run document, and the only writer of it (S10.3, S10.4).

`artifacts/bench/latest.json` is read by three things that are nowhere near the code
that wrote it -- `report.py --compare`, `--write-baseline`, and the CI artifact upload
-- so the shape it must have is stated once, as data, in `bench_result.schema.json`,
and checked here.

Two decisions are load-bearing:

* **Validate before the destination is opened.** :func:`write_result` validates the
  whole document first and only then touches the filesystem, so a rejected run leaves
  *no* file rather than a truncated one. A half-written `latest.json` is what makes
  `--compare` fail on a file that cannot exist, and the failure surfaces in the next
  job rather than in the one that produced it.
* **The schema subset is interpreted here rather than by a library.** S2.1 pins no
  `jsonschema` distribution and its last rule makes the dependency set closed, so
  adding one is a spec amendment rather than a code change. The document uses only
  ``type`` / ``required`` / ``properties`` / ``items`` / ``enum`` / ``minimum`` /
  ``minItems``, and :func:`validate_bench_result` implements exactly those; a schema
  keyword it does not know is an error rather than a silent pass, so the subset cannot
  quietly grow into an unchecked one.

Errors carry a JSON-pointer-ish path (``/phases/0/wall_ms``) because "wrong type" with
no location is unactionable in a document this deep, and every violation is reported
rather than just the first: a run that is missing four fingerprint fields should cost
one CI round trip, not four.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from pathlib import Path
from typing import Any, Final

__all__ = [
    "BenchResultError",
    "SCHEMA_PATH",
    "canonical_json",
    "load_schema",
    "validate_bench_result",
    "write_result",
]

#: The committed contract. Read rather than restated: a schema duplicated in Python
#: is a schema that drifts from the one `--validate-baselines` and any future consumer
#: would read.
SCHEMA_PATH: Final[Path] = Path(__file__).resolve().parent / "bench_result.schema.json"

#: The keywords :func:`validate_bench_result` understands. Anything else in the schema
#: raises, so the checker can never be weaker than the document it is graded against.
SUPPORTED_KEYWORDS: Final[frozenset[str]] = frozenset(
    {
        "$schema",
        "$id",
        "title",
        "description",
        "type",
        "required",
        "properties",
        "items",
        "enum",
        "minimum",
        "minItems",
    }
)

#: JSON Schema type names, mapped onto what `json.load` actually produces. `bool` is
#: excluded from `number` and `integer` deliberately: Python's `True` is an `int`, and
#: a `wall_ms` of `true` must not validate as a measurement.
_TYPES: Final[Mapping[str, tuple[type, ...]]] = {
    "object": (dict,),
    "array": (list,),
    "string": (str,),
    "number": (int, float),
    "integer": (int,),
    "boolean": (bool,),
    "null": (type(None),),
}


class BenchResultError(Exception):
    """A benchmark document, or `scales.yaml`, violated its contract.

    Raised by `scales.py` as well: one error type for "the benchmark harness was
    handed something it cannot measure against" keeps the S9.2 preflight and the
    S10.3 writer from needing two `except` clauses for the same class of mistake.
    """


def load_schema(path: Path | str = SCHEMA_PATH) -> Mapping[str, Any]:
    """Read `bench_result.schema.json`."""
    schema = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(schema, dict):
        raise BenchResultError(f"{path}: a JSON Schema must be an object")
    return schema


def canonical_json(document: Any) -> str:
    """The one byte-for-byte rendering of a run document.

    Sorted keys and a fixed indent so two runs that measured the same thing produce
    the same bytes, and a trailing newline so the file is a well-formed text file for
    `git diff` when a baseline is committed through a reviewed PR (S10.3).
    """
    return json.dumps(document, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def validate_bench_result(
    document: Any, schema: Mapping[str, Any] | None = None
) -> Mapping[str, Any]:
    """Check `document` against the run-document schema.

    Returns:
        The document, so a caller can validate inline.

    Raises:
        BenchResultError: listing every violation, each with its path in the document.
    """
    resolved = load_schema() if schema is None else schema
    violations: list[str] = []
    _check(document, resolved, "", violations)
    if violations:
        raise BenchResultError(
            "benchmark result document is invalid:\n  " + "\n  ".join(violations)
        )
    return document


def write_result(document: Any, path: Path | str, schema: Mapping[str, Any] | None = None) -> Path:
    """Validate `document`, then write it to `path` as canonical JSON.

    Nothing is opened, created or truncated until validation has passed, so a
    rejected document leaves the destination exactly as it was -- absent, when it was
    absent. Returns the path written.
    """
    validate_bench_result(document, schema)
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(canonical_json(document), encoding="utf-8")
    return destination


def _check(value: Any, schema: Mapping[str, Any], pointer: str, violations: list[str]) -> None:
    """Append every way `value` fails `schema` to `violations`."""
    unknown = sorted(frozenset(schema) - SUPPORTED_KEYWORDS)
    if unknown:
        raise BenchResultError(
            f"{_at(pointer)}: schema uses keywords this validator does not implement: "
            f"{', '.join(unknown)}"
        )

    expected = schema.get("type")
    if expected is not None:
        allowed = _TYPES.get(expected)
        if allowed is None:
            raise BenchResultError(f"{_at(pointer)}: schema names unknown type '{expected}'")
        if isinstance(value, bool) and expected in ("number", "integer"):
            violations.append(f"{_at(pointer)}: expected {expected}, found boolean")
            return
        if not isinstance(value, allowed):
            violations.append(f"{_at(pointer)}: expected {expected}, found {type(value).__name__}")
            return

    if "enum" in schema and value not in schema["enum"]:
        violations.append(f"{_at(pointer)}: {value!r} is not one of {schema['enum']}")

    if "minimum" in schema and isinstance(value, int | float) and value < schema["minimum"]:
        violations.append(f"{_at(pointer)}: {value} is below the minimum {schema['minimum']}")

    if isinstance(value, dict):
        for key in schema.get("required", ()):
            if key not in value:
                violations.append(f"{_at(pointer)}: missing required key '{key}'")
        properties: Mapping[str, Any] = schema.get("properties", {})
        for key, subschema in properties.items():
            if key in value:
                _check(value[key], subschema, f"{pointer}/{key}", violations)

    if isinstance(value, list) and not isinstance(value, str):
        if len(value) < schema.get("minItems", 0):
            violations.append(
                f"{_at(pointer)}: {len(value)} item(s), fewer than the required "
                f"{schema['minItems']}"
            )
        item_schema = schema.get("items")
        if isinstance(item_schema, Mapping):
            for index, item in enumerate(value):
                _check(item, item_schema, f"{pointer}/{index}", violations)


def _at(pointer: str) -> str:
    return pointer or "<document>"
