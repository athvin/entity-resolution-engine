"""Compare full-prediction execution settings using one fitted model and corpus."""

from __future__ import annotations

import os
from collections.abc import Callable
from pathlib import Path
from typing import Any

from ulid import ULID

from er.lake.ducklake import connect


def run_matrix(directory: Path, command: Callable[..., dict[str, Any]]) -> list[dict[str, Any]]:
    projection = "rec_a_key, rec_b_key, match_probability, evidence"
    scores = f"SELECT {projection} FROM lake.main.match_scores WHERE is_active"
    reference = directory / "matrix-reference.parquet"
    with connect() as connection:
        connection.execute(f"COPY ({scores}) TO ? (FORMAT PARQUET)", [str(reference)])
    names = (
        "ER_SPLINK_MATERIALISATION",
        "ER_SPLINK_NUM_CHUNKS_LEFT",
        "ER_SPLINK_NUM_CHUNKS_RIGHT",
        "ER_SPLINK_WORK_DIR",
    )
    previous = {name: os.environ.get(name) for name in names}
    try:
        results = []
        for materialisation in ("table", "parquet"):
            for chunks in (1, 2, 4):
                scratch = directory / f"scratch-{materialisation}-{chunks}"
                os.environ["ER_SPLINK_MATERIALISATION"] = materialisation
                os.environ["ER_SPLINK_NUM_CHUNKS_LEFT"] = str(chunks)
                os.environ["ER_SPLINK_NUM_CHUNKS_RIGHT"] = str(chunks)
                if materialisation == "parquet":
                    os.environ["ER_SPLINK_WORK_DIR"] = str(scratch)
                else:
                    os.environ.pop("ER_SPLINK_WORK_DIR", None)
                entry = command(["er", "match", "--mode", "full"], str(ULID()), "matrix")
                with connect() as connection:
                    # Exact relational equality, including log-space evidence. Ignore
                    # only run IDs and timestamps that necessarily change on a rescore.
                    expected = f"SELECT {projection} FROM read_parquet(?)"
                    different = connection.execute(
                        f"SELECT count(*) FROM (({scores} EXCEPT {expected}) UNION ALL "
                        f"({expected} EXCEPT {scores}))",
                        [str(reference), str(reference)],
                    ).fetchone()
                if different != (0,):
                    raise AssertionError(
                        f"prediction matrix changed scores: {materialisation}/{chunks}"
                    )
                if list(scratch.rglob("*.parquet")):
                    raise AssertionError(f"prediction left scratch files: {scratch}")
                results.append(
                    {
                        "materialisation": materialisation,
                        "chunks_left": chunks,
                        "chunks_right": chunks,
                        "seconds": entry["duration_ms"] / 1000,
                        "cpu": entry["cpu"],
                        "identical_scores": True,
                        "scratch_clean": True,
                    }
                )
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
    return results
