"""Preserve stored review evidence when Splink emits log2 weights."""

import json
import math
from pathlib import Path

import duckdb
import pytest

from er.config.loader import load_config
from er.errors import StageFailure
from er.matching.evidence import build_evidence


@pytest.mark.parametrize("weight_columns", [False, True])
def test_evidence_preserves_factors_including_zero_infinity_and_null(weight_columns: bool) -> None:
    cfg = load_config(Path(__file__).resolve().parents[3] / "configs/test.yaml")
    values: dict[str, float | int | None] = {"match_weight": 2.5}
    expected: dict[str, float | int | None] = {"match_weight": 2.5}
    weights = (-math.inf, -3.0, 0.0, 2.0, math.inf, None)
    for (column, spec), weight in zip(cfg.comparisons.items(), weights, strict=True):
        values[f"gamma_{column}"] = expected[f"gamma_{column}"] = 1
        factor = None if weight is None else 2.0**weight
        expected[f"bf_{column}"] = factor
        values[f"{'mw' if weight_columns else 'bf'}_{column}"] = (
            weight if weight_columns else factor
        )
        if spec.tf:
            expected[f"bf_tf_adj_{column}"] = 0.25
            values[f"{'mw' if weight_columns else 'bf'}_tf_adj_{column}"] = (
                -2.0 if weight_columns else 0.25
            )

    source = ", ".join(f"? AS {column}" for column in values)
    expression = build_evidence(cfg, values.keys())
    with duckdb.connect() as connection:
        row = connection.execute(
            f"SELECT {expression} FROM (SELECT {source})", list(values.values())
        ).fetchone()
    assert row is not None
    assert json.loads(row[0]) == expected


def test_evidence_still_refuses_missing_comparison_factors() -> None:
    cfg = load_config(Path(__file__).resolve().parents[3] / "configs/test.yaml")
    available = ["match_weight", *(f"gamma_{column}" for column in cfg.comparisons)]
    with pytest.raises(StageFailure, match="bf_given_name"):
        build_evidence(cfg, available)
