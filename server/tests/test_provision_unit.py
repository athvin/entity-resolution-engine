"""Provisioning derivation, config seeding, and the provision job's fault map.

Bare units: no Postgres, no lake. The seams that need services are covered by
``test_provision_pg.py`` (control-plane flows) and ``test_provision_e2e.py``
(the real dedicated database).
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

import psycopg
import pytest
import yaml
from erserver import provision
from erserver.settings import ServerSettings

from er.lake.init import DataPathMismatchError

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_TEMPLATE = REPO_ROOT / "configs" / "default.yaml"


def settings(**overrides: Any) -> ServerSettings:
    values: dict[str, Any] = {
        "dsn": "postgresql://unused/cp",
        "maint_dsn": "postgresql://maint@cluster:5432/postgres",
        "tenant_dsn_template": "postgresql://er:pw@catalog:5432/{dbname}",
        "lake_data_path_template": "s3://lake/{ns}/",
        "config_root": "/var/lib/er/configs",
        "drop_root": "/var/lib/er/drop",
    }
    values.update(overrides)
    return ServerSettings(**values)


# --------------------------------------------------------------------------- #
# derivation
# --------------------------------------------------------------------------- #


def test_namespace_is_deterministic_and_identifier_safe() -> None:
    ns = provision.tenant_namespace("acme")
    assert ns == provision.tenant_namespace("acme")
    assert re.fullmatch(r"t_[a-z0-9_]+", ns)
    assert len(provision.tenant_db_name("acme")) <= 63
    assert provision.tenant_db_name("acme") == f"er_{ns}"


def test_orgs_that_normalize_identically_stay_distinct() -> None:
    assert provision.tenant_namespace("a-b") != provision.tenant_namespace("a_b")


def test_long_org_names_truncate_without_colliding() -> None:
    prefix = "x" * 40
    first, second = f"{prefix}alpha", f"{prefix}beta"
    assert provision.tenant_namespace(first) != provision.tenant_namespace(second)
    assert len(provision.tenant_db_name(first)) <= 63


# --------------------------------------------------------------------------- #
# plan_for
# --------------------------------------------------------------------------- #


def test_plan_derives_the_whole_tenant_identity() -> None:
    plan = provision.plan_for(settings(tenant_env_extra={"ER_S3_ENDPOINT": "minio:9000"}), "acme")
    assert plan.tenant == provision.tenant_namespace("acme")
    assert plan.db_name == provision.tenant_db_name("acme")
    assert plan.catalog_dsn == f"postgresql://er:pw@catalog:5432/{plan.db_name}"
    assert plan.data_path == f"s3://lake/{plan.tenant}/"
    assert plan.config_path == "/var/lib/er/configs/acme.yaml"
    assert plan.drop_root == "/var/lib/er/drop/acme"
    assert plan.env["ER_CATALOG_DSN"] == plan.catalog_dsn
    assert plan.env["ER_LAKE_DATA_PATH"] == plan.data_path
    assert plan.env["ER_LAKE_METADATA_SCHEMA"] == plan.tenant
    assert plan.env["ER_S3_ENDPOINT"] == "minio:9000"


def test_identity_keys_win_over_the_shared_extras() -> None:
    plan = provision.plan_for(
        settings(tenant_env_extra={"ER_CATALOG_DSN": "postgresql://wrong/db"}), "acme"
    )
    assert plan.env["ER_CATALOG_DSN"] == plan.catalog_dsn


@pytest.mark.parametrize(
    ("override", "named"),
    [
        ({"maint_dsn": None}, "ERSERVER_MAINT_DSN"),
        ({"tenant_dsn_template": None}, "ERSERVER_TENANT_DSN_TEMPLATE"),
        ({"tenant_dsn_template": "postgresql://er@host/fixed"}, "ERSERVER_TENANT_DSN_TEMPLATE"),
        ({"lake_data_path_template": "s3://lake/flat"}, "ERSERVER_LAKE_DATA_PATH_TEMPLATE"),
        ({"config_root": None}, "ERSERVER_CONFIG_ROOT"),
        ({"drop_root": None}, "ERSERVER_DROP_ROOT"),
    ],
)
def test_plan_refuses_naming_the_missing_setting(override: dict[str, Any], named: str) -> None:
    with pytest.raises(provision.ProvisioningNotConfigured, match=named):
        provision.plan_for(settings(**override), "acme")


# --------------------------------------------------------------------------- #
# seed config rendering
# --------------------------------------------------------------------------- #


def test_render_templates_the_reference_document_and_validates() -> None:
    plan = provision.plan_for(settings(), "acme")
    rendered = provision.render_seed_config(DEFAULT_TEMPLATE.read_text(), plan)
    document = yaml.safe_load(rendered)
    assert document["tenant"] == plan.tenant
    assert document["storage"]["data_path"] == plan.data_path
    assert document["storage"]["drop_dir"] == plan.drop_root
    assert document["storage"]["model_uri_prefix"] == f"{plan.data_path}models/"


def test_render_refuses_a_template_that_is_not_an_s6_document() -> None:
    plan = provision.plan_for(settings(), "acme")
    with pytest.raises(provision.ProvisioningNotConfigured, match="not an S6 document"):
        provision.render_seed_config("just: a mapping\n", plan)


def test_missing_template_names_the_setting() -> None:
    with pytest.raises(provision.ProvisioningNotConfigured, match="ERSERVER_CONFIG_TEMPLATE"):
        provision.seed_template_text(settings(config_template_path="/nowhere/template.yaml"))


# --------------------------------------------------------------------------- #
# the provision job's fault taxonomy (execute, steps monkeypatched)
# --------------------------------------------------------------------------- #

PARAMS = {"tenant": "t_acme_0a1b2c3d", "db_name": "er_t_acme_0a1b2c3d", "data_path": "s3://l/t/"}
RUN_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHJ"


def _noop_create(maint_dsn: str, db_name: str) -> bool:
    return True


def test_missing_maint_dsn_is_a_config_failure(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(provision.MAINT_DSN_VAR, raising=False)
    outcome = provision.execute(PARAMS, run_id=RUN_ID)
    assert (outcome.exit_code, outcome.error_class) == (2, "config")
    assert outcome.stages[-1].stage == "create_database"


def test_unreachable_cluster_is_transient(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(provision.MAINT_DSN_VAR, "postgresql://maint@cluster/postgres")
    monkeypatch.setattr(
        provision,
        "create_tenant_database",
        lambda dsn, name: (_ for _ in ()).throw(psycopg.OperationalError("connection refused")),
    )
    outcome = provision.execute(PARAMS, run_id=RUN_ID)
    assert (outcome.exit_code, outcome.error_class) == (1, "transient_io")


def test_missing_createdb_privilege_is_permanent_config(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(provision.MAINT_DSN_VAR, "postgresql://maint@cluster/postgres")
    monkeypatch.setattr(
        provision,
        "create_tenant_database",
        lambda dsn, name: (_ for _ in ()).throw(
            psycopg.errors.InsufficientPrivilege("permission denied to create database")
        ),
    )
    outcome = provision.execute(PARAMS, run_id=RUN_ID)
    assert (outcome.exit_code, outcome.error_class) == (2, "config")


def test_data_path_mismatch_is_the_engines_precondition(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv(provision.MAINT_DSN_VAR, "postgresql://maint@cluster/postgres")
    monkeypatch.setattr(provision, "create_tenant_database", _noop_create)
    monkeypatch.setattr(
        "er.lake.init.init_lake",
        lambda **kwargs: (_ for _ in ()).throw(
            DataPathMismatchError(catalog="s3://l/old/", env="s3://l/new/", tenant="t")
        ),
    )
    outcome = provision.execute(PARAMS, run_id=RUN_ID)
    assert (outcome.exit_code, outcome.error_class) == (3, "precondition")
    assert [stage.stage for stage in outcome.stages] == ["create_database", "init_lake"]
    assert outcome.stages[0].status == "succeeded"


def test_success_reports_both_stages_and_emits_s52_lines(
    monkeypatch: pytest.MonkeyPatch, capfd: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv(provision.MAINT_DSN_VAR, "postgresql://maint@cluster/postgres")
    monkeypatch.setattr(provision, "create_tenant_database", _noop_create)
    monkeypatch.setattr("er.lake.init.init_lake", lambda **kwargs: ())
    outcome = provision.execute(PARAMS, run_id=RUN_ID)
    assert (outcome.exit_code, outcome.mode, outcome.run_id) == (0, "provision", RUN_ID)
    assert [stage.status for stage in outcome.stages] == ["succeeded", "succeeded"]
    lines = [json.loads(line) for line in capfd.readouterr().err.strip().splitlines()]
    assert [line["stage"] for line in lines] == ["create_database", "init_lake"]
    assert all("status" in line and "duration_ms" in line for line in lines)
