"""Unit tests for the single dbt entry point (S4.0b, S4.2, S4.6, S6; S8.4).

Every assertion here is against something the spec states rather than something the
implementation happens to do: the config document — not ``dbt_project.yml`` — is
the source of every var, no entity-id list may travel in argv, ``--full-refresh``
is never implicit, and no Python DuckDB connection spans the subprocess.

Nothing here spawns dbt, opens a connection or needs Docker. The subprocess is a
spy and the connection is the pair of callables ``run_dbt`` takes precisely so this
ordering is checkable without a lake.
"""

from __future__ import annotations

import json
import re
import subprocess
from collections.abc import Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import pytest
import yaml

from er.config.loader import load_config
from er.dbt_runner import (
    DBT_PROFILES_DIR,
    DBT_PROJECT_DIR,
    MAX_VARS_BYTES,
    render_dbt_vars,
    run_dbt,
)
from er.errors import ConfigError, ExitCode, StageFailure, exit_code_for

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"
PROJECT = REPO_ROOT / "dbt" / "dbt_project.yml"

# A literal 26-character ULID. Used as the run id, and as the element that makes a
# list an entity-id list for the S4.6 ban.
RUN_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHJ"
OTHER_ULID = "01JQZ8XKQ4T7VN3M2B9CDEFGHK"

#: S6's `standardization` block: exactly three keys, consumed by `email_norm` and
#: `phone_e164` (S4.2) and by nothing else.
STANDARDIZATION_KEYS = {
    "email_strip_plus_addressing",
    "email_placeholders",
    "phone_default_region",
}

#: AC1's key set. A sixth key here without a spec row is the defect this pins.
VAR_KEYS = {
    "std_version",
    "survivorship_version",
    "run_id",
    "sources",
    "standardization",
    # S4.6 dispatches each attribute's rule chain from the config, so the marts are
    # handed the `survivorship:` block whole -- the same reason `sources` is carried
    # whole rather than projected (ER-088).
    "survivorship",
}

# AC4's ceiling on the whole payload for configs/test.yaml. Deliberately far below
# er.dbt_runner.MAX_VARS_BYTES, which is itself far below Linux's 128 KiB per-argv
# limit: this asserts the real config renders small, not merely that it is legal.
TEST_CONFIG_VARS_BUDGET = 8192

ULID_RE = re.compile(r"[0-7][0-9A-HJKMNP-TV-Z]{25}")


@dataclass
class Spy:
    """A subprocess, and the connection around it, recorded as one event log.

    The event order IS the S4.0b assertion: `close` before `spawn`, `reopen` after
    it, and the snapshot probe on the two sides where a connection exists.
    """

    exit_code: int = 0
    stdout: str = ""
    stderr: str = ""
    argv: list[str] = field(default_factory=list)
    events: list[str] = field(default_factory=list)
    snapshots: list[int] = field(default_factory=lambda: [7, 9])

    def __call__(self, argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
        self.argv = list(argv)
        self.events.append("spawn")
        return subprocess.CompletedProcess(list(argv), self.exit_code, self.stdout, self.stderr)

    def close(self) -> None:
        self.events.append("close")

    def reopen(self) -> None:
        self.events.append("reopen")

    def probe(self) -> int:
        self.events.append("probe")
        return self.snapshots[min(self.events.count("probe") - 1, len(self.snapshots) - 1)]


def config() -> Any:
    return load_config(TEST_CONFIG)


def var_payload(argv: Sequence[str]) -> dict[str, Any]:
    """The decoded `--vars` element of an argv."""
    index = list(argv).index("--vars")
    decoded = json.loads(argv[index + 1])
    assert isinstance(decoded, dict)
    return decoded


def test_render_dbt_vars_key_set_and_values() -> None:
    """AC1: exactly S6's five vars, every value taken from the document."""
    cfg = config()
    payload = render_dbt_vars(cfg, RUN_ID)

    assert set(payload) == VAR_KEYS

    assert payload["std_version"] == cfg.versions.std_version
    assert payload["survivorship_version"] == cfg.versions.survivorship_version
    assert payload["run_id"] == RUN_ID

    standardization = payload["standardization"]
    assert isinstance(standardization, dict)
    assert set(standardization) == STANDARDIZATION_KEYS
    assert standardization["email_strip_plus_addressing"] == (
        cfg.standardization.email_strip_plus_addressing
    )
    assert standardization["email_placeholders"] == cfg.standardization.email_placeholders
    assert standardization["phone_default_region"] == cfg.standardization.phone_default_region

    # S4.2's stg_<source> models read the column mapping from this var, and S4.6's
    # `source_priority` rule reads priority_rank from it.
    sources = payload["sources"]
    assert isinstance(sources, dict)
    assert set(sources) == set(cfg.sources)
    for name, spec in cfg.sources.items():
        assert sources[name]["columns"] == spec.columns
        assert sources[name]["priority_rank"] == spec.priority_rank
        assert sources[name]["record_id_column"] == spec.record_id_column
        assert sources[name]["date_format"] == spec.date_format


def test_project_vars_are_all_overridden() -> None:
    """AC2: no dbt var is ever left to its dbt_project.yml fallback at runtime."""
    project = yaml.safe_load(PROJECT.read_text(encoding="utf-8"))
    declared = set(project.get("vars") or {})
    assert declared, "dbt_project.yml declares no vars; the parity check would be vacuous"

    cfg = config()
    payload = render_dbt_vars(cfg, RUN_ID)
    missing = sorted(declared - set(payload))
    assert not missing, (
        f"dbt_project.yml declares {missing} with no config source, so a runtime "
        f"invocation would silently take the fallback; S6 is the source of truth"
    )

    # And the override wins on value, not merely on presence: each declared var
    # must equal the document's field, whatever dbt_project.yml happens to say. The
    # source block differs per var — S4.2's staging models read `sources` and
    # `standardization`, which are not `versions` fields — so the document's value is
    # looked up per var rather than resolved against one block.
    from_document: dict[str, object] = {
        "std_version": cfg.versions.std_version,
        "survivorship_version": cfg.versions.survivorship_version,
        "standardization": cfg.standardization.model_dump(),
        "sources": {name: spec.model_dump() for name, spec in cfg.sources.items()},
        # S4.6's per-attribute rule chains, carried whole so `golden_records` can
        # dispatch them (ER-088). Listed here for the same reason every other var is:
        # this test is what proves the runtime override beats the dbt_project.yml
        # fallback, and a var absent from this mapping is one it cannot check.
        "survivorship": {attribute: list(chain) for attribute, chain in cfg.survivorship.items()},
    }
    for key in sorted(declared):
        assert key in from_document, (
            f"dbt_project.yml declares '{key}' with no S6 block named here, so this "
            f"test could not tell an override from a fallback"
        )
        assert payload[key] == from_document[key]


def test_extra_cannot_shadow_config_vars() -> None:
    """AC3: `extra` adds payloads; it can never overwrite a config-sourced var."""
    cfg = config()

    with pytest.raises(ConfigError) as excinfo:
        render_dbt_vars(cfg, RUN_ID, extra={"std_version": "x"})
    assert "std_version" in str(excinfo.value)
    # An invalid invocation, so exit 2 rather than 1 (S4.0, S4.7).
    assert exit_code_for(excinfo.value) == ExitCode.CONFIG

    blocking = [{"key_type": "email_exact", "expr": "email"}]
    merged = render_dbt_vars(cfg, RUN_ID, extra={"blocking": blocking})
    assert merged["blocking"] == blocking
    assert set(merged) == VAR_KEYS | {"blocking"}
    assert merged["std_version"] == cfg.versions.std_version


def test_vars_payload_never_carries_an_entity_id_list(tmp_path: Path) -> None:
    """AC4: the payload stays small, and an id list is refused rather than truncated."""
    payload = render_dbt_vars(config(), RUN_ID)
    encoded = json.dumps(payload)
    assert len(encoded.encode("utf-8")) < TEST_CONFIG_VARS_BUDGET

    def id_lists(value: object) -> list[object]:
        if isinstance(value, str):
            return []
        if isinstance(value, dict):
            return [found for item in value.values() for found in id_lists(item)]
        if isinstance(value, list):
            if any(isinstance(item, str) and ULID_RE.fullmatch(item) for item in value):
                return [value]
            return [found for item in value for found in id_lists(item)]
        return []

    assert not id_lists(payload), "a list of ULIDs in the payload is the S4.6 E2BIG defect"

    # The ban is enforced, not merely observed: the touched set travels through
    # er_touched_entities, so a stage that tries to pass ids is refused.
    with pytest.raises(ConfigError) as excinfo:
        render_dbt_vars(config(), RUN_ID, extra={"entity_ids": [RUN_ID, OTHER_ULID]})
    assert "entity_ids" in str(excinfo.value)

    spy = Spy()
    with pytest.raises(ConfigError):
        run_dbt("run", vars={"touched": [RUN_ID]}, spawn=spy, artifacts_dir=tmp_path)
    with pytest.raises(ConfigError):
        run_dbt("run", vars={"blob": "x" * MAX_VARS_BYTES}, spawn=spy, artifacts_dir=tmp_path)
    # Refused before the fork, which is the only way the E2BIG never happens.
    assert spy.events == []


def test_argv_construction_and_full_refresh_is_explicit(tmp_path: Path) -> None:
    """AC5: the exact S4.2 argv, and --full-refresh only when asked for."""
    spy = Spy()
    run_dbt(
        "run",
        select="staging+ intermediate",
        target="lake",
        spawn=spy,
        artifacts_dir=tmp_path,
    )
    # `--vars` is passed on every invocation (S6); with no payload that is the
    # empty object, never a dropped flag. The quotes S4.2 writes around the JSON
    # are shell quoting -- there is no shell here, so the element is the raw JSON.
    assert spy.argv == [
        "dbt",
        "run",
        "--project-dir",
        DBT_PROJECT_DIR,
        "--profiles-dir",
        DBT_PROFILES_DIR,
        "--target",
        "lake",
        "--select",
        "staging+ intermediate",
        "--vars",
        "{}",
    ]
    assert "--full-refresh" not in spy.argv

    # S4.2: the absence of --changed-only widens the SELECTION and never implies a
    # rebuild, so an unselected run still carries no --full-refresh.
    wide = Spy()
    payload = render_dbt_vars(config(), RUN_ID)
    run_dbt("run", vars=payload, target="lake", spawn=wide, artifacts_dir=tmp_path)
    assert "--select" not in wide.argv
    assert "--full-refresh" not in wide.argv
    assert var_payload(wide.argv) == payload

    rebuild = Spy()
    run_dbt(
        "run",
        select="staging+ intermediate",
        vars=payload,
        target="lake",
        full_refresh=True,
        spawn=rebuild,
        artifacts_dir=tmp_path,
    )
    assert rebuild.argv.count("--full-refresh") == 1
    assert rebuild.argv[-1] == "--full-refresh"

    # dbt's own concurrency is pinned in profiles.yml (S4.0b), never on argv.
    assert "--threads" not in rebuild.argv


def test_connection_is_closed_before_and_reopened_after_dbt(tmp_path: Path) -> None:
    """AC6: no Python DuckDB connection spans the subprocess, on either path."""
    ordered = ["probe", "close", "spawn", "reopen", "probe"]

    ok = Spy()
    result = run_dbt(
        "run",
        vars=render_dbt_vars(config(), RUN_ID),
        close_conn=ok.close,
        reopen_conn=ok.reopen,
        snapshot_probe=ok.probe,
        spawn=ok,
        artifacts_dir=tmp_path,
    )
    assert ok.events == ordered
    # The snapshot range is read on the two sides where a connection exists.
    assert (result.snapshot_start, result.snapshot_end) == (7, 9)

    failing = Spy(exit_code=1)
    with pytest.raises(StageFailure):
        run_dbt(
            "run",
            vars=render_dbt_vars(config(), RUN_ID),
            close_conn=failing.close,
            reopen_conn=failing.reopen,
            snapshot_probe=failing.probe,
            spawn=failing,
            artifacts_dir=tmp_path,
        )
    # `reopen` still ran: a failed dbt invocation must not leave the stage without
    # the connection it has to write its run_stages row with.
    assert failing.events[:4] == ordered[:4]
    assert "reopen" in failing.events


def test_non_zero_dbt_exit_maps_to_exit_code_1_and_captures_the_log(tmp_path: Path) -> None:
    """AC7: a failure exits 1 with the output on disk; a success parses run_results."""
    failing = Spy(exit_code=2, stdout="Runtime Error in model int_std_records", stderr="boom")
    with pytest.raises(StageFailure) as excinfo:
        run_dbt(
            "run",
            vars=render_dbt_vars(config(), RUN_ID),
            spawn=failing,
            artifacts_dir=tmp_path,
        )
    # S4.7 classifies a dbt failure as `data`, which exits 1 -- never dbt's own 2,
    # which S4.0 reserves for a config or usage error.
    assert exit_code_for(excinfo.value) == ExitCode.STAGE_FAILURE

    logs = sorted((tmp_path / "dbt").glob("*.log"))
    assert len(logs) == 1
    captured = logs[0].read_text(encoding="utf-8")
    assert "Runtime Error in model int_std_records" in captured
    assert "boom" in captured
    assert str(logs[0]) in str(excinfo.value)
    # Named for the run it belongs to, so an operator can find it from a log line.
    assert RUN_ID in logs[0].name

    project_dir = tmp_path / "project"
    (project_dir / "target").mkdir(parents=True)
    (project_dir / "target" / "run_results.json").write_text(
        json.dumps(
            {
                "results": [
                    {
                        "unique_id": "model.er.int_std_records",
                        "status": "success",
                        "execution_time": 1.5,
                        "adapter_response": {"rows_affected": 10},
                    },
                    {"unique_id": "model.er.int_blocking_keys", "status": "success"},
                ]
            }
        ),
        encoding="utf-8",
    )

    ok = Spy()
    result = run_dbt(
        "run",
        vars=render_dbt_vars(config(), RUN_ID),
        spawn=ok,
        project_dir=str(project_dir),
        artifacts_dir=tmp_path,
    )
    assert result.exit_code == 0
    assert [model.unique_id for model in result.models] == [
        "model.er.int_std_records",
        "model.er.int_blocking_keys",
    ]
    assert result.models[0].rows_affected == 10
    assert result.models[1].rows_affected is None
    assert all(model.status == "success" for model in result.models)
    assert result.log_path.exists()
    # A second invocation under one run_id -- standardize then assemble -- keeps
    # its own log rather than overwriting the first.
    assert result.log_path != logs[0]
