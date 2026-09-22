"""The S10.4 environment fingerprint: what a measured run was taken on (M2, M24).

A benchmark number is only comparable against another taken on the same substrate, so
every result carries the environment it was measured in. The two values a reader most
often gets wrong are read here, once, the way S10.4 states:

* **CPU is `cpu.max` as quota ÷ period, never `nproc`.** `deploy.resources.limits.cpus`
  is a CFS quota, and `nproc` reports the host's cores — on an 8-core host a container
  limited to 2 CPUs still sees `nproc == 8`. :func:`read_cgroup_cpu_max` parses the
  quota/period pair; `"max <period>"` (unlimited) is `None`.
* **Tool versions come from the installed distributions**, not from a committed list, so
  a result cannot claim a pin the image does not actually carry (S2.1).

The module is import-light and its parsers are pure, so the arithmetic is unit-tested
without a container; `environment_fingerprint` is exercised by the smoke integration run,
where the cgroup files and the DuckDB extension actually exist.
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from importlib import metadata
from pathlib import Path
from typing import Any

import duckdb

__all__ = [
    "environment_fingerprint",
    "read_cgroup_cpu_max",
    "read_cgroup_memory_max",
]

#: The cgroup v2 files S10.4 reads. A path rather than a literal so the smoke run can
#: point the reader at the real hierarchy and a unit test at a fixture.
_CGROUP_ROOT = Path("/sys/fs/cgroup")

_UNLIMITED = "max"


def read_cgroup_cpu_max(text: str) -> float | None:
    """Parse a cgroup v2 `cpu.max` string to the CPU count it allows.

    `cpu.max` is ``"<quota> <period>"`` in microseconds, or ``"max <period>"`` when the
    controller is unlimited. Returns ``quota / period`` (e.g. ``"200000 100000" -> 2.0``)
    and ``None`` for the unlimited case — never a core count, which is a different number
    (S10.4).

    Raises:
        ValueError: the string is not the two-token shape cgroup v2 writes.
    """
    parts = text.split()
    if len(parts) != 2:
        raise ValueError(f"cpu.max is not '<quota> <period>': {text!r}")
    quota, period = parts
    if quota == _UNLIMITED:
        return None
    divisor = int(period)
    if divisor == 0:
        raise ValueError(f"cpu.max period is zero: {text!r}")
    return int(quota) / divisor


def read_cgroup_memory_max(text: str) -> int | None:
    """Parse cgroup v2 `memory.max`: an integer of bytes, or ``None`` for ``"max"``."""
    stripped = text.strip()
    return None if stripped == _UNLIMITED else int(stripped)


def _read_file(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return None


def _cpu_max_field(cgroup_root: Path) -> str:
    """`cpu.max` as the schema's string field: the allowed CPU count, or ``"max"``."""
    text = _read_file(cgroup_root / "cpu.max")
    if text is None:
        return _UNLIMITED
    allowed = read_cgroup_cpu_max(text)
    return _UNLIMITED if allowed is None else str(allowed)


def _memory_field(cgroup_root: Path, name: str) -> int:
    """A cgroup memory byte count, 0 when the file is absent or unlimited (S10.4)."""
    text = _read_file(cgroup_root / name)
    if text is None:
        return 0
    value = read_cgroup_memory_max(text)
    return 0 if value is None else value


def _ducklake_extension_version(connection: duckdb.DuckDBPyConnection) -> str:
    row = connection.execute(
        "SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'ducklake'"
    ).fetchone()
    return "" if row is None or row[0] is None else str(row[0])


def environment_fingerprint(
    *,
    scale: str,
    connection: duckdb.DuckDBPyConnection,
    config_hash: str,
    generator_seed: int,
    model_version: str,
    tf_snapshot_id: str,
    cgroup_root: Path = _CGROUP_ROOT,
    environ: Mapping[str, str] | None = None,
) -> dict[str, Any]:
    """Assemble the S10.4 fingerprint block for a run, reading the substrate at runtime.

    `cgroup_root` and `environ` are injectable so the arithmetic is testable off a real
    container; everything else is read from the process and the lake. The keys are
    exactly the schema's required set and nothing more.
    """
    env = os.environ if environ is None else environ
    try:
        from er.matching.runtime import MatchingRuntime
    except ModuleNotFoundError as error:
        if error.name != "er.matching.runtime":
            raise
        matching_runtime = {"supported": False}
    else:
        matching_runtime = MatchingRuntime.from_env(env).fingerprint()

    return {
        "matching_runtime": matching_runtime,
        "scale": scale,
        "image_digest": env.get("ER_IMAGE_DIGEST", "unknown"),
        "git_sha": env.get("ER_GIT_SHA", "unknown"),
        "runner": env.get("ER_RUNNER", "local"),
        "cgroup_cpu_max": _cpu_max_field(cgroup_root),
        "cgroup_memory_max": _memory_field(cgroup_root, "memory.max"),
        "cgroup_memory_peak": _memory_field(cgroup_root, "memory.peak"),
        "nproc": os.cpu_count() or 0,
        "er_duckdb_threads": int(env.get("ER_DUCKDB_THREADS", "0")),
        "er_duckdb_memory_limit": env.get("ER_DUCKDB_MEMORY_LIMIT", ""),
        "duckdb_version": metadata.version("duckdb"),
        "splink_version": metadata.version("splink"),
        "dbt_core_version": metadata.version("dbt-core"),
        "dbt_duckdb_version": metadata.version("dbt-duckdb"),
        "ducklake_extension_version": _ducklake_extension_version(connection),
        "config_hash": config_hash,
        "generator_seed": generator_seed,
        "model_version": model_version,
        "tf_snapshot_id": tf_snapshot_id,
    }
