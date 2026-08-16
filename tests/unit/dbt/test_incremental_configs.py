"""The S4.2 incremental configuration, asserted over every dbt model (S4.2, S5.1).

S4.2 states the configuration "one line per model family" and then says the words
this module exists to make executable: `on_schema_change` MUST be
`append_new_columns` on every incremental model, no model declares `indexes`, and
no dbt model uses `incremental_strategy='merge'`. Those are properties of the whole
`dbt/models/` tree rather than of any one model, so they are checked by reading the
tree rather than by a per-model assertion each new model would have to remember to
copy.

Three things make this a real check rather than a grep that happens to pass:

* The scan is **anchored**. `EXPECTED_INCREMENTAL_MODELS` names the models that must
  be found, so a rename, a move or a `model-paths` change fails here instead of
  quietly reducing the suite to zero files.
* The `indexes` and `merge` bans cover `dbt_project.yml` as well. Both are settable
  at the project root, where a per-file scan would never see them, and a `+indexes`
  there would be exactly as fatal to DuckLake as one written in a model.
* The `stg_*` predicate is asserted as the literal S4.2 writes, `{{ this }}` and
  all. It is what makes `er standardize` idempotent (M16): a re-delivered batch is
  recognised by its id and appends nothing, while the `>` watermark it is so easy to
  reach for would re-append every row of a batch whose `ingested_at` values tie.

`append_new_columns` and not `sync_all_columns`: S5.0 enforces a contract on every
dbt-owned model and dbt-core 1.12.2 rejects any other value on a contract-enforced
incremental model. The additive `std_version`-bump column S4.2 and S5.1 want
propagated is exactly what `append_new_columns` propagates; a column removal or
retype now fails the contract loudly instead of being synced away.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest
import yaml

REPO_ROOT = Path(__file__).resolve().parents[3]
DBT_DIR = REPO_ROOT / "dbt"
MODELS_DIR = DBT_DIR / "models"
PROJECT_FILE = DBT_DIR / "dbt_project.yml"

#: The models this ticket's milestone ships, so the scan below cannot pass by
#: finding nothing. Later milestones add `int_*` and `golden_*`; each is held to the
#: same rules by the same scan the moment its file lands.
EXPECTED_INCREMENTAL_MODELS = frozenset({"stg_crm", "stg_billing", "stg_webforms"})

#: S4.2's `stg_<source>` family: `append`, because a staged row is a projection of an
#: append-only `raw_records` version and is never updated in place (D7).
STAGING_STRATEGY = "append"

#: The value S4.2 mandates on EVERY incremental model.
ON_SCHEMA_CHANGE = "append_new_columns"

#: S4.2's predicate, verbatim. Whitespace-insensitive only between tokens: the shape
#: is the contract, and `{{ this }}` is part of it.
BATCH_PREDICATE = "ingest_batch_id not in (select distinct ingest_batch_id from {{ this }})"

_CONFIG_BLOCK_RE = re.compile(r"\{\{\s*config\((?P<body>.*?)\)\s*\}\}", re.DOTALL)


def model_files() -> list[Path]:
    """Every dbt model in the project, whatever milestone put it there."""
    return sorted(path for path in MODELS_DIR.rglob("*") if path.suffix in (".sql", ".py"))


def config_body(source: str) -> str:
    """The text inside the model's `{{ config(...) }}` call, or `''` if it has none."""
    match = _CONFIG_BLOCK_RE.search(source)
    return match.group("body") if match else ""


def config_value(body: str, key: str) -> str | None:
    """The literal a `config()` keyword is set to, or None when it is not set."""
    match = re.search(rf"{key}\s*=\s*'([^']*)'", body)
    return match.group(1) if match else None


def collapse(text: str) -> str:
    """`text` with every run of whitespace reduced to one space.

    The predicate below is wrapped across lines in the model for readability; what
    S4.2 fixes is the token sequence, not the line breaks.
    """
    return re.sub(r"\s+", " ", text).strip()


def incremental_models() -> dict[str, str]:
    """Every model whose config materializes it incrementally, name -> source."""
    found = {}
    for path in model_files():
        source = path.read_text(encoding="utf-8")
        if config_value(config_body(source), "materialized") == "incremental":
            found[path.stem] = source
    return found


def test_models_directory_holds_the_expected_incremental_models() -> None:
    """The anchor: a scan that finds nothing must not report success."""
    assert EXPECTED_INCREMENTAL_MODELS <= set(incremental_models()), (
        f"expected {sorted(EXPECTED_INCREMENTAL_MODELS)} under {MODELS_DIR}; "
        f"found {sorted(incremental_models())}"
    )


@pytest.mark.parametrize("name", sorted(EXPECTED_INCREMENTAL_MODELS))
def test_every_incremental_model_syncs_all_columns(name: str) -> None:
    """AC6: every incremental model sets `on_schema_change` to S4.2's value.

    The node id is the ticket's and predates the amendment that replaced
    `sync_all_columns`; what it asserts is the amended value, which is the one that
    can coexist with the S5.0 contract.
    """
    source = incremental_models()[name]
    assert config_value(config_body(source), "on_schema_change") == ON_SCHEMA_CHANGE, (
        f"{name}: S4.2 requires on_schema_change='{ON_SCHEMA_CHANGE}' on every "
        f"incremental model, so a std_version bump's additive column propagates"
    )


@pytest.mark.parametrize("name", sorted(EXPECTED_INCREMENTAL_MODELS))
def test_staging_uses_append_with_batch_predicate(name: str) -> None:
    """AC6: the `stg_*` family is `append`, guarded by the not-in-distinct-batch predicate."""
    source = incremental_models()[name]
    body = config_body(source)

    assert config_value(body, "incremental_strategy") == STAGING_STRATEGY, (
        f"{name}: S4.2 gives the stg_<source> family incremental_strategy='{STAGING_STRATEGY}'"
    )
    assert "is_incremental()" in source, (
        f"{name}: the predicate must be guarded by is_incremental()"
    )
    assert collapse(BATCH_PREDICATE) in collapse(source), (
        f"{name}: S4.2's predicate is `{BATCH_PREDICATE}` verbatim. A `>` watermark "
        f"on ingested_at re-appends every row of a batch whose stamps tie, which is "
        f"exactly the idempotence M16 asks for"
    )


def test_no_model_declares_indexes_or_merge() -> None:
    """AC6: DuckLake has no indexes, and dbt's merge strategy cannot express its MERGE.

    `dbt_project.yml` is read alongside the models because both settings can be
    declared at the project root, where no per-model scan would ever see them.
    """
    for path in model_files():
        where = path.relative_to(REPO_ROOT)
        body = config_body(path.read_text(encoding="utf-8"))
        assert not re.search(r"\bindexes\b", body), (
            f"{where}: S4.2 -- no model declares `indexes`; DuckLake has none"
        )
        assert config_value(body, "incremental_strategy") != "merge", (
            f"{where}: S4.2 -- no dbt model uses incremental_strategy='merge'; "
            f"DuckLake's MERGE supports a single when_matched action, which dbt's "
            f"merge strategy cannot express"
        )

    # Both settings are declarable at the project root, where a per-model scan would
    # never see them, and either one there would apply to every model at once.
    project_source = PROJECT_FILE.read_text(encoding="utf-8")
    assert not re.search(r"^\s*\+?indexes\s*:", project_source, re.M), (
        f"{PROJECT_FILE.relative_to(REPO_ROOT)}: a project-level `indexes` applies to "
        f"every model; DuckLake has no indexes (S4.2)"
    )
    assert not re.search(r"incremental_strategy\s*:\s*merge", project_source), (
        f"{PROJECT_FILE.relative_to(REPO_ROOT)}: a project-level merge strategy would "
        f"apply to every model (S4.2)"
    )

    # The project-level default is the same value, so a future model that omits the
    # keyword inherits the mandated one rather than dbt-duckdb's `ignore`.
    project = yaml.safe_load(PROJECT_FILE.read_text(encoding="utf-8"))
    assert project["models"]["+on_schema_change"] == ON_SCHEMA_CHANGE
