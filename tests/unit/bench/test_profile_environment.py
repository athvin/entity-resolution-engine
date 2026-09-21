"""Failed profiling setup must leave an in-process caller's lake settings intact."""

import importlib
import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "benchmarks"))
profile_pipeline = importlib.import_module("profile_pipeline")


@pytest.mark.parametrize("detailed", [False, True])
def test_failed_profile_restores_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, detailed: bool
) -> None:
    monkeypatch.setenv("ER_CONFIG", "caller.yaml")
    monkeypatch.setenv("ER_LAKE_METADATA_SCHEMA", "caller_schema")
    monkeypatch.setenv("ER_LAKE_DATA_PATH", "s3://lake/caller/")
    monkeypatch.delenv("ER_PROFILE_SESSION_ID", raising=False)
    before = {key: value for key, value in os.environ.items() if key.startswith("ER_")}
    with pytest.raises(FileNotFoundError):
        profile_pipeline.run_case(
            tmp_path,
            "tiny",
            1,
            detailed=detailed,
            config_template=tmp_path / "missing.yaml",
        )
    assert {key: value for key, value in os.environ.items() if key.startswith("ER_")} == before
