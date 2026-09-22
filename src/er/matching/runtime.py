"""Execution-only Splink controls; none changes blocking or model parameters."""

from __future__ import annotations

import os
from collections.abc import Mapping
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Literal, cast

from er.errors import ConfigError


@dataclass(frozen=True)
class MatchingRuntime:
    num_chunks_left: int = 1
    num_chunks_right: int = 1
    materialisation: Literal["table", "parquet"] = "table"
    materialisation_dir: str | None = None

    @classmethod
    def from_env(cls, environ: Mapping[str, str] | None = None) -> MatchingRuntime:
        env = os.environ if environ is None else environ
        chunks: list[int] = []
        for side in ("LEFT", "RIGHT"):
            name = f"ER_SPLINK_NUM_CHUNKS_{side}"
            value = env.get(name, "1")
            if not value.isascii() or not value.isdigit() or int(value) < 1:
                raise ConfigError(f"{name} must be a positive integer")
            chunks.append(int(value))
        mode = env.get("ER_SPLINK_MATERIALISATION", "table")
        if mode not in ("table", "parquet"):
            raise ConfigError("ER_SPLINK_MATERIALISATION must be table or parquet")
        directory = env.get("ER_SPLINK_WORK_DIR") or None
        if mode == "parquet":
            if directory is None or "://" in directory or not Path(directory).is_absolute():
                raise ConfigError(
                    "Parquet materialisation requires an absolute local ER_SPLINK_WORK_DIR"
                )
        elif directory is not None:
            raise ConfigError("ER_SPLINK_WORK_DIR is only valid with Parquet materialisation")
        return cls(chunks[0], chunks[1], cast(Literal["table", "parquet"], mode), directory)

    def fingerprint(self) -> dict[str, object]:
        return asdict(self)
