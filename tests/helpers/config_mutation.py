"""Write a temp copy of a config YAML with dotted-path overrides (S6, S8.2.1).

A test that needs the pipeline to run against a *changed* config must never edit the
committed `configs/test.yaml` in place: a failure between the edit and its restore would
leave the tree dirty and fail the gate ladder for an unrelated reason. This helper reads
a config, applies `{dotted.path: value}` overrides onto the parsed document, and writes a
fresh file under a caller-chosen directory. `load_config` then reads it like any other.

The round-trip is through `yaml.safe_load`/`yaml.safe_dump`; `config_hash` is insensitive
to key order and re-serialisation (S8.4), so a mutated copy that changes no value hashes
identically to the original — which is what lets a test assert that the *only* thing it
moved is the override.
"""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
from typing import Any

import yaml

__all__ = ["mutated_config"]


def mutated_config(
    base_config: Path,
    overrides: Mapping[str, Any],
    *,
    dest_dir: Path,
    name: str = "mutated.yaml",
) -> Path:
    """Return a path to a temp config equal to ``base_config`` with ``overrides`` applied.

    Args:
        base_config: the committed config to copy; never modified.
        overrides: ``{dotted.path: value}``, e.g. ``{"sources.crm.priority_rank": 2}``.
            Every path segment before the last must name an existing mapping — a typo'd
            path is a test defect, so it raises rather than silently creating a key the
            loader would then reject or ignore.
        dest_dir: where to write the copy; created if absent. A caller's ``tmp_path``.
        name: the file name under ``dest_dir``.

    Returns:
        The path to the written copy.

    Raises:
        KeyError: a dotted path traverses a key that is not an existing mapping.
    """
    document: Any = yaml.safe_load(base_config.read_text(encoding="utf-8"))
    for dotted, value in overrides.items():
        segments = dotted.split(".")
        cursor: Any = document
        for segment in segments[:-1]:
            if not isinstance(cursor, dict) or segment not in cursor:
                raise KeyError(
                    f"override path {dotted!r} traverses {segment!r}, which is not a key "
                    f"of the document at that level"
                )
            cursor = cursor[segment]
        leaf = segments[-1]
        if not isinstance(cursor, dict) or leaf not in cursor:
            raise KeyError(f"override path {dotted!r} sets {leaf!r}, which the document lacks")
        cursor[leaf] = value

    dest_dir.mkdir(parents=True, exist_ok=True)
    destination = dest_dir / name
    destination.write_text(yaml.safe_dump(document, sort_keys=True), encoding="utf-8")
    return destination
