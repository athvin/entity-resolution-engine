"""The dbt project and profile are asserted against DesignDoc.md, not against themselves.

S4.0b renders the connection contract twice — once as the SQL statement block
``lake/ducklake.py`` executes, once as the ``lake`` target of this profile — and says
the two MUST agree field for field. That is the one place the Python and dbt
renderings can drift, so the parity test below parses the spec's own YAML block and
compares, rather than restating it here.

The remaining tests hold the three properties a service-less runner depends on:
every ``env_var()`` carries a default (S9.1), ``mem`` attaches nothing, and dbt's
``threads`` is not DuckDB's.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any

import yaml

from er.lake.model import DBT_OWNED

REPO_ROOT = Path(__file__).resolve().parents[2]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"
DBT_DIR = REPO_ROOT / "dbt"
PROFILES = DBT_DIR / "profiles" / "profiles.yml"
PROJECT = DBT_DIR / "dbt_project.yml"
PACKAGES = DBT_DIR / "packages.yml"
PACKAGE_LOCK = DBT_DIR / "package-lock.yml"

PROFILE_NAME = "er"

# S9.1: exactly two targets, no third.
EXPECTED_TARGETS = {"lake", "mem"}

# AC4's literal restatement of the S4.0b `settings:` block. The parity test below
# is what actually proves the agreement; this set is the guard that a spec block
# reformatted into something unparseable cannot quietly empty that test.
EXPECTED_SETTINGS_KEYS = {
    "extension_directory",
    "autoinstall_known_extensions",
    "autoload_known_extensions",
    "threads",
    "memory_limit",
}
EXPECTED_SECRET_OPTION_KEYS = {"key_id", "secret", "endpoint", "url_style", "use_ssl", "region"}

# Every variable the profile is allowed to read, and therefore every one that must
# carry a default. A new name here without a default fails AC8's regex below.
EXPECTED_ENV_VARS = {
    "ER_DUCKDB_THREADS",
    "ER_DUCKDB_MEMORY_LIMIT",
    "ER_S3_ACCESS_KEY_ID",
    "ER_S3_SECRET_ACCESS_KEY",
    "ER_S3_ENDPOINT",
    "ER_S3_URL_STYLE",
    "ER_S3_USE_SSL",
    "ER_S3_REGION",
    "ER_CATALOG_DSN",
    "ER_LAKE_DATA_PATH",
    "ER_LAKE_METADATA_SCHEMA",
}

ENV_VAR_CALL_RE = re.compile(r"env_var\(\s*'([^']+)'\s*(,)?\s*([^)]*)\)")

# An exact pin, never a range: `>=`, `<`, `~`, `^` or a comma-separated pair.
EXACT_VERSION_RE = re.compile(r"^\d+\.\d+\.\d+$")


def section(anchor: str) -> str:
    text = DESIGN_DOC.read_text(encoding="utf-8")
    start = text.find(f'<a id="{anchor}"></a>')
    assert start != -1, f"DesignDoc.md has no anchor {anchor}"
    end = text.find('<a id="', start + 1)
    return text[start:end] if end != -1 else text[start:]


def spec_lake_target() -> dict[str, Any]:
    """The `lake:` YAML block S4.0b declares the dbt profile MUST equal."""
    block = re.search(r"```yaml\n(lake:\n.*?)```", section("s4-0b"), re.DOTALL)
    assert block is not None, "S4.0b no longer carries a fenced yaml block starting at `lake:`"
    parsed = yaml.safe_load(block.group(1))
    assert isinstance(parsed, dict) and "lake" in parsed
    target = parsed["lake"]
    assert isinstance(target, dict)
    return target


def profile() -> dict[str, Any]:
    document = yaml.safe_load(PROFILES.read_text(encoding="utf-8"))
    assert isinstance(document, dict), "profiles.yml did not parse to a mapping"
    assert set(document) == {PROFILE_NAME}, (
        f"profiles.yml must declare exactly the '{PROFILE_NAME}' profile, found {sorted(document)}"
    )
    return dict(document[PROFILE_NAME])


def outputs() -> dict[str, dict[str, Any]]:
    targets = profile()["outputs"]
    assert set(targets) == EXPECTED_TARGETS, (
        f"S9.1 defines exactly {sorted(EXPECTED_TARGETS)}; found {sorted(targets)}"
    )
    return dict(targets)


def test_lake_target_matches_s4_0b_field_for_field() -> None:
    lake = outputs()["lake"]
    assert lake == spec_lake_target(), (
        "the `lake` target and the S4.0b yaml block have diverged; they are the "
        "same connection contract rendered twice and MUST agree field for field"
    )

    # AC4, restated against the parsed profile so a spec block that stopped
    # parsing cannot make the comparison above vacuous.
    settings = lake["settings"]
    assert set(settings) == EXPECTED_SETTINGS_KEYS
    assert settings["extension_directory"] == "/opt/duckdb_extensions"
    assert settings["autoinstall_known_extensions"] is False
    assert settings["autoload_known_extensions"] is False
    assert "ER_DUCKDB_THREADS" in settings["threads"]
    assert "ER_DUCKDB_MEMORY_LIMIT" in settings["memory_limit"]

    assert len(lake["secrets"]) == 1
    secret = dict(lake["secrets"][0])
    assert secret.pop("name") == "er_s3"
    assert secret.pop("type") == "s3"
    assert set(secret) == EXPECTED_SECRET_OPTION_KEYS

    assert len(lake["attach"]) == 1
    attach = lake["attach"][0]
    assert attach["path"] == "ducklake:postgres:{{ env_var('ER_CATALOG_DSN', '') }}"
    assert attach["alias"] == "lake"
    assert set(attach["options"]) == {"data_path", "metadata_schema"}


def test_mem_target_has_no_attach_or_extensions() -> None:
    mem = outputs()["mem"]
    assert mem["type"] == "duckdb"
    assert mem["path"] == ":memory:"
    # `mem` exists so `dbt parse` and `dbt compile` run on a bare service-less
    # runner (S8.1). An `attach:` would need a catalog and an `extensions:` would
    # need the baked extension directory, neither of which such a runner has.
    assert "attach" not in mem
    assert "extensions" not in mem


def profile_source() -> str:
    """profiles.yml with whole-line comments dropped.

    The prose above the profile names `env_var()` while explaining the rule, and a
    regex check that counted those mentions would be measuring the comment rather
    than the configuration.
    """
    lines = PROFILES.read_text(encoding="utf-8").splitlines()
    return "\n".join(line for line in lines if not line.lstrip().startswith("#"))


def test_every_env_var_call_has_a_default() -> None:
    text = profile_source()
    calls = ENV_VAR_CALL_RE.findall(text)
    names = {name for name, _, _ in calls}
    assert names == EXPECTED_ENV_VARS, (
        f"profiles.yml reads {sorted(names)}; expected {sorted(EXPECTED_ENV_VARS)}"
    )
    # A missing default is the M23 defect: the static job cannot parse the project
    # because it has no services and therefore no ER_* variables set.
    undefaulted = sorted(name for name, comma, default in calls if not comma or not default.strip())
    assert not undefaulted, f"env_var() calls with no default: {undefaulted}"

    assert text.count("env_var(") == len(calls), (
        "an env_var( call did not match the parser above; it may carry no default"
    )


def test_threads_is_one_on_both_targets() -> None:
    targets = outputs()
    for name, target in sorted(targets.items()):
        # dbt's own concurrency, pinned to 1 by S4.0b so no dbt invocation opens a
        # second connection to a single-writer lake.
        assert target["threads"] == 1, f"target '{name}' must declare threads: 1"

    # DuckDB's threads is a different key with a different source. Conflating them
    # would either uncap dbt or hard-pin the engine to one thread.
    duckdb_threads = targets["lake"]["settings"]["threads"]
    assert duckdb_threads != targets["lake"]["threads"]
    assert "env_var('ER_DUCKDB_THREADS'" in duckdb_threads


def test_project_sets_contract_and_on_schema_change() -> None:
    project = yaml.safe_load(PROJECT.read_text(encoding="utf-8"))
    assert project["profile"] == PROFILE_NAME
    assert project["model-paths"] == ["models"]

    models = project["models"]
    # At the `models:` root, so no future model can silently omit either one.
    assert models["+contract"] == {"enforced": True}
    # append_new_columns, not sync_all_columns: dbt-core 1.12.2 raises
    # ValidationError for any other value once `contract.enforced` is set on an
    # incremental model, so this pair is the only one the two S4.2/S5.0 rules can
    # both satisfy. Asserted together, because the constraint is on the PAIR.
    assert models["+on_schema_change"] == "append_new_columns"

    # The two version vars, the two the S4.2 staging models read AT RENDER TIME, and
    # the S4.6 survivorship chains the marts dispatch: a missing var is tolerated by
    # `dbt parse` and is a hard error in `dbt compile --target mem`, and S9.1 runs both
    # on a runner with no services and no config document.
    #
    # Exact, not a subset. The rule the exactness encodes is that every var declared
    # here must be one the pipeline actually overrides -- otherwise it is a real
    # fallback rather than a rendering aid, and a run could silently read it.
    # `test_dbt_runner.py::test_project_vars_are_all_overridden` is the other half of
    # that pair, and it checks each of these resolves to an S6 field at runtime.
    assert set(project["vars"]) == {
        "std_version",
        "survivorship_version",
        "sources",
        "standardization",
        "survivorship",
    }

    # S6: the CLI's `--vars` override always wins, so these two are fallbacks for a
    # bare `dbt parse` / `dbt compile --target mem` and nothing else. The note
    # saying so is load-bearing: without it the next reader treats dbt_project.yml
    # as a second source of truth for a version the pipeline never reads from here.
    lines = PROJECT.read_text(encoding="utf-8").splitlines()
    vars_line = next(i for i, line in enumerate(lines) if line.startswith("vars:"))
    preamble = [line for line in lines[:vars_line][::-1] if line.startswith("#")]
    assert any("fallback" in line.lower() for line in preamble), (
        "dbt_project.yml's `vars:` block needs a comment marking the two versions as "
        "fallbacks only; S6 is the source of truth and the CLI --vars override wins"
    )


def test_every_shipped_model_materializes_a_dbt_owned_relation() -> None:
    # S12: M1 shipped no dbt model at all — the first dbt-owned relation does not
    # exist until M2 — and that milestone has now landed the S4.2 staging models, so
    # the empty-set form of this assertion has been overtaken by the board.
    #
    # What survives it is the rule the M1 wording was protecting: a model file
    # materializes a relation, and S5.0 gives every relation exactly two possible
    # owners. A model named outside `DBT_OWNED` is a relation `ddl.py` believes it
    # owns and dbt would create anyway, which is the ownership collision S5.0 exists
    # to prevent. Source and test declarations (`schema.yml`) are not models and are
    # deliberately allowed.
    model_files = sorted(
        path for path in (DBT_DIR / "models").rglob("*") if path.suffix in (".sql", ".py")
    )
    strays = sorted(
        path.relative_to(REPO_ROOT).as_posix() for path in model_files if path.stem not in DBT_OWNED
    )
    assert not strays, f"models materializing relations S5 does not declare dbt-owned: {strays}"


def test_dbt_utils_is_pinned_exactly_and_locked() -> None:
    packages = yaml.safe_load(PACKAGES.read_text(encoding="utf-8"))["packages"]
    pins = {entry["package"]: str(entry["version"]) for entry in packages}
    assert "dbt-labs/dbt_utils" in pins, "dbt_utils supplies unique_combination_of_columns (S5.0)"
    for package, version in sorted(pins.items()):
        assert EXACT_VERSION_RE.match(version), f"{package} is pinned to a range: {version!r}"

    # The lock is committed, so `dbt deps` resolves the same tree on every runner.
    lock = yaml.safe_load(PACKAGE_LOCK.read_text(encoding="utf-8"))
    locked = {entry["package"]: str(entry["version"]) for entry in lock["packages"]}
    assert locked == pins, f"package-lock.yml resolves {locked}, packages.yml pins {pins}"
