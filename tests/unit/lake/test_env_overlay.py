"""The :func:`er.lake.env.lake_environment` overlay: per-context, merge, restore."""

from __future__ import annotations

import threading

import pytest

from er.lake.env import MissingEnvError, lake_environment, require_env, require_int_env


def test_overlay_wins_and_process_env_answers_the_rest(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("ER_LAKE_METADATA_SCHEMA", "process_schema")
    monkeypatch.setenv("ER_CATALOG_DSN", "postgresql://shared")

    with lake_environment({"ER_LAKE_METADATA_SCHEMA": "t_acme"}):
        assert require_env("ER_LAKE_METADATA_SCHEMA") == "t_acme"
        assert require_env("ER_CATALOG_DSN") == "postgresql://shared"
    assert require_env("ER_LAKE_METADATA_SCHEMA") == "process_schema"


def test_missing_stays_missing_inside_an_overlay(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("ER_DUCKDB_THREADS", raising=False)
    with lake_environment({"ER_LAKE_DATA_PATH": "s3://lake/t_acme/"}):
        with pytest.raises(MissingEnvError, match="ER_DUCKDB_THREADS"):
            require_env("ER_DUCKDB_THREADS")


def test_overlay_values_are_validated_like_env_values(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("ER_DUCKDB_THREADS", raising=False)
    with lake_environment({"ER_DUCKDB_THREADS": "4"}):
        assert require_int_env("ER_DUCKDB_THREADS") == 4


def test_nested_overlays_shadow_and_restore(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("ER_LAKE_DATA_PATH", raising=False)
    with lake_environment({"ER_LAKE_DATA_PATH": "s3://lake/outer/"}):
        with lake_environment({"ER_LAKE_DATA_PATH": "s3://lake/inner/"}):
            assert require_env("ER_LAKE_DATA_PATH") == "s3://lake/inner/"
        assert require_env("ER_LAKE_DATA_PATH") == "s3://lake/outer/"
    with pytest.raises(MissingEnvError):
        require_env("ER_LAKE_DATA_PATH")


def test_overlay_does_not_leak_across_threads(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ER_LAKE_METADATA_SCHEMA", "process_schema")
    seen: dict[str, str] = {}

    def other_thread() -> None:
        seen["value"] = require_env("ER_LAKE_METADATA_SCHEMA")

    with lake_environment({"ER_LAKE_METADATA_SCHEMA": "t_acme"}):
        worker = threading.Thread(target=other_thread)
        worker.start()
        worker.join()
    # A fresh thread starts with a fresh context: the overlay is request-scoped,
    # never ambiently process-scoped.
    assert seen["value"] == "process_schema"


def test_empty_overlay_value_is_missing_not_blank(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ER_S3_REGION", "us-east-1")
    with lake_environment({"ER_S3_REGION": "  "}):
        with pytest.raises(MissingEnvError, match="ER_S3_REGION"):
            require_env("ER_S3_REGION")
