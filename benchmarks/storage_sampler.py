"""Sample local temporary storage separately from persistent lake files."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from er.obs.profiling import ResourceSampler


def file_bytes(directory: Path, pattern: str) -> int:
    total = 0
    for path in directory.glob(pattern):
        try:
            if path.is_file():
                total += path.stat().st_size
        except FileNotFoundError:
            pass  # A stage may release a temporary file between glob and stat.
    return total


class StorageSampler(ResourceSampler):
    def _sample(self) -> dict[str, Any]:
        sample = super()._sample()
        # DuckDB's default in-memory spill location is relative to the process
        # working directory. CLI and dbt execute from these two image directories.
        sample["spill_bytes"] = sum(
            file_bytes(root, "duckdb_temp*") for root in (Path("/app/.tmp"), Path("/app/dbt/.tmp"))
        )
        directory = os.environ.get("ER_SPLINK_WORK_DIR")
        sample["parquet_scratch_bytes"] = (
            file_bytes(Path(directory), "**/*.parquet") if directory else 0
        )
        return sample
