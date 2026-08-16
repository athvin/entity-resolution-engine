"""The committed fixture model, checked without training anything (S4.3.2, S8.3).

T-TRAIN-1 is the test that proves the committed artifacts are what the six pinned
inputs produce, and it costs minutes: a ten-thousand-record corpus, an ingest, a dbt
build and a full EM fit. It is `slow`-marked and excluded from the PR path for that
reason, which would leave the artifact every M3 scenario loads unguarded on every
ordinary change — so everything about it that can be checked by *reading* it is
checked here, on every PR:

* the sidecar's `sha256` really is the committed model's, so the three files are one
  artifact set rather than three files that were copied at different times;
* the sidecar's `training:` block equals `configs/test.yaml`'s, in both directions —
  the pinned input a config edit moves first;
* the frozen TF rows cover exactly the `tf: true` columns of S6, which is what stops
  Splink computing the rest from the corpus at hand (D4, INV-SCORE);
* the settings document is bracketed as S4.3.1 requires — `NullLevel` first,
  `ElseLevel` last — because a comparison with no else level yields a NULL match
  weight for any pair that matches nothing, silently;
* `load_fixture_model` installs the model without invoking a single Splink estimator,
  asserted against a spy over Splink's own training namespace;
* and the two divergence reports `--check` is built from name the right thing, which
  is the half of T-TRAIN-1's contract that does not need a fit to exercise.

The lake here is a plain in-memory DuckDB with `main` attached as `lake` and the two
relations created from the S5 registry. `load_fixture_model` writes rows and reads
nothing else, so a real DuckLake would add a service dependency to the unit layer
(S8.1 runs it on a bare runner) without adding an assertion.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import subprocess
import sys
import tomllib
from collections.abc import Iterator
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
import yaml
from helpers.model import (
    FIXTURE_META_PATH,
    FIXTURE_MODEL_PATH,
    FIXTURE_MODEL_VERSION,
    FIXTURE_TF_SNAPSHOT_ID,
    fixture_meta,
    fixture_settings,
    fixture_tf_rows,
    load_fixture_model,
)
from splink.internals.linker_components.training import LinkerTraining

from er.config.loader import load_config
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER, create_table_sql
from er.lake.model_registry import ACTIVE
from er.matching.tf import tf_columns

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
CONFIG_PATH: Final = REPO_ROOT / "configs" / "test.yaml"
FIXTURE_DIR: Final = REPO_ROOT / "fixtures" / "static"
CI_WORKFLOW: Final = REPO_ROOT / ".github" / "workflows" / "ci.yaml"
PYPROJECT: Final = REPO_ROOT / "pyproject.toml"
REPRODUCIBILITY_TEST: Final = REPO_ROOT / "tests" / "integration" / "test_train_reproducibility.py"

#: The two relations `load_fixture_model` writes, both `ddl.py`-owned (S5).
LAKE_RELATIONS: Final[tuple[str, ...]] = ("model_registry", "tf_lookup")

#: S6's `tf: true` columns for `configs/test.yaml`. Spelled out because AC5 is an
#: equality against a literal set and deriving both sides from the config would assert
#: only that the derivation is a function.
TF_COLUMNS: Final[frozenset[str]] = frozenset({"given_name", "family_name", "email"})

#: Every estimator on Splink's training namespace. The spy patches all of them, not
#: just S4.3.2's three: "no Splink training method is invoked" is the claim, and
#: `estimate_m_from_label_column` would be training just as much.
SPLINK_ESTIMATORS: Final[tuple[str, ...]] = tuple(
    name for name in dir(LinkerTraining) if name.startswith("estimate_")
)


def regen_module() -> Any:
    """`scripts/regen_fixture_model.py`, loaded by path (see the T-TRAIN-1 module)."""
    path = REPO_ROOT / "scripts" / "regen_fixture_model.py"
    spec = importlib.util.spec_from_file_location("regen_fixture_model", path)
    assert spec is not None and spec.loader is not None, f"cannot load {path}"
    module = importlib.util.module_from_spec(spec)
    # Registered before execution: the script's `@dataclass` fields are string
    # annotations, and `dataclasses` resolves them via `sys.modules[cls.__module__]`.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def lake() -> Iterator[duckdb.DuckDBPyConnection]:
    """An in-memory stand-in for `lake.main`, holding the two relations S5 declares."""
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    for relation in LAKE_RELATIONS:
        connection.execute(create_table_sql(REGISTRY[relation]))
    try:
        yield connection
    finally:
        connection.close()


@pytest.fixture
def refuse_training(monkeypatch: pytest.MonkeyPatch) -> list[str]:
    """A spy over Splink's training namespace that records and refuses every estimator."""
    called: list[str] = []

    def refusing(estimator: str) -> Any:
        def refuse(*args: object, **kwargs: object) -> None:
            called.append(estimator)
            raise AssertionError(
                f"scenario tests never train, and {estimator} was called (S4.3.2 item 6)"
            )

        return refuse

    for name in SPLINK_ESTIMATORS:
        monkeypatch.setattr(LinkerTraining, name, refusing(name))
    return called


def test_meta_sidecar_matches_committed_artifacts() -> None:
    """AC3: the sidecar describes the two files committed beside it, not older ones."""
    meta = fixture_meta()
    committed = FIXTURE_MODEL_PATH.read_bytes()

    assert meta["sha256"] == hashlib.sha256(committed).hexdigest(), (
        f"{FIXTURE_META_PATH.name} pins a SHA-256 that is not {FIXTURE_MODEL_PATH.name}'s; "
        f"the three artifacts were not written by one regeneration"
    )
    assert meta["artifact"] == FIXTURE_MODEL_PATH.name
    assert json.loads(committed.decode("utf-8")), "the committed settings document is empty"

    # The version and TF snapshot `tests/helpers/model.py` writes into `model_registry`
    # are the ones the artifact was fitted under. They are literals there so that
    # module imports on a tree with no fixture yet; this is what keeps them true.
    assert meta["model_version"] == FIXTURE_MODEL_VERSION
    assert meta["tf_snapshot_id"] == FIXTURE_TF_SNAPSHOT_ID

    rows = fixture_tf_rows()
    assert meta["tf_rows"] == len(rows), "the sidecar's row count is not the tf.csv's"
    assert rows, "the committed model froze no TF rows, so a scenario would compute them"
    assert {(row[0], row[1]) for row in rows} == {(FIXTURE_MODEL_VERSION, FIXTURE_TF_SNAPSHOT_ID)}


def test_training_block_in_meta_equals_config() -> None:
    """AC3: the pinned `training:` block is `configs/test.yaml`'s, in both directions.

    Both directions because the interesting failure is asymmetric: a key ADDED to the
    config that the sidecar does not carry is a model fitted under a setting nobody
    recorded, and a subset comparison would call that agreement.
    """
    document = yaml.safe_load(CONFIG_PATH.read_text(encoding="utf-8"))
    pinned = fixture_meta()["training"]

    assert pinned == document["training"]
    assert document["training"] == pinned
    assert set(pinned) == set(document["training"])

    # And it is the block the loader validates, not merely the text of it: S4.3.2
    # drives every estimator argument off this block.
    assert pinned == load_config(CONFIG_PATH).training.model_dump()


def test_tf_csv_covers_exactly_the_tf_true_columns() -> None:
    """AC5: the frozen rows cover the `tf: true` columns of S6 and no others.

    A subset would be worse than nothing: Splink computes what it is not given, so a
    missing column's term frequencies would come from the scenario corpus at hand and
    the resulting `match_probability` would be neither the frozen value nor a
    reproducible one (D4, S4.3.3).
    """
    committed = {row[2] for row in fixture_tf_rows()}

    assert committed == TF_COLUMNS
    assert committed == set(tf_columns(load_config(CONFIG_PATH)))
    assert all(row[4] > 0.0 for row in fixture_tf_rows()), (
        "a frozen tf_value of zero is not a value"
    )


def test_settings_declare_record_key_and_bracketed_levels() -> None:
    """AC6: `unique_id_column_name` is `record_key`, and every comparison is bracketed.

    S4.3.1 makes the bracketing normative: without a `NullLevel` first the null case
    falls into whichever level's SQL happens to match, and without an `ElseLevel` last
    Splink emits a `CASE … END` with no `ELSE`, so a pair matching nothing gets gamma
    NULL and a NULL match weight — a silent hole rather than a failure. D6 makes
    `record_key` the identity Splink joins on.
    """
    settings = fixture_settings()

    assert settings["unique_id_column_name"] == "record_key"

    comparisons = settings["comparisons"]
    assert len(comparisons) == len(load_config(CONFIG_PATH).comparisons) == 6

    for comparison in comparisons:
        levels = comparison["comparison_levels"]
        name = comparison.get("output_column_name")
        assert levels[0].get("is_null_level") is True, f"{name}: first level is not the null level"
        assert str(levels[-1]["sql_condition"]).strip().upper() == "ELSE", (
            f"{name}: last level is not the else level"
        )


def test_load_fixture_model_installs_frozen_rows_and_one_active_row(
    lake: duckdb.DuckDBPyConnection, refuse_training: list[str]
) -> None:
    """AC4, AC5: the committed rows land, exactly one row is active, nothing trains."""
    model_version, tf_snapshot_id, settings = load_fixture_model(lake)

    assert (model_version, tf_snapshot_id) == (FIXTURE_MODEL_VERSION, FIXTURE_TF_SNAPSHOT_ID)
    assert settings == fixture_settings()
    assert refuse_training == [], "loading a committed model invoked a Splink estimator"

    loaded = lake.execute(
        f"SELECT model_version, tf_snapshot_id, column_name, value, tf_value "
        f"FROM {SCHEMA_QUALIFIER}.tf_lookup ORDER BY column_name, value"
    ).fetchall()
    assert [tuple(row) for row in loaded] == [tuple(row) for row in fixture_tf_rows()]

    columns = {
        str(name)
        for (name,) in lake.execute(
            f"SELECT DISTINCT column_name FROM {SCHEMA_QUALIFIER}.tf_lookup"
        ).fetchall()
    }
    assert columns == TF_COLUMNS

    registry = lake.execute(
        f"SELECT model_version, status, params_path, tf_snapshot_id, metrics "
        f"FROM {SCHEMA_QUALIFIER}.model_registry"
    ).fetchall()
    assert len(registry) == 1, registry
    version, status, params_path, snapshot, metrics = registry[0]
    assert (version, status, snapshot) == (FIXTURE_MODEL_VERSION, ACTIVE, FIXTURE_TF_SNAPSHOT_ID)
    assert params_path == str(FIXTURE_MODEL_PATH)
    assert json.loads(metrics)["training"] == fixture_meta()["training"]

    # Twice is once: a scenario may load the model in a fixture and again in a helper,
    # and appending would give every TF value two frequencies.
    load_fixture_model(lake)
    assert lake.execute(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.tf_lookup").fetchone() == (
        len(fixture_tf_rows()),
    )
    assert lake.execute(
        f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.model_registry WHERE status = ?", [ACTIVE]
    ).fetchone() == (1,)


def test_check_names_the_artifact_whose_bytes_moved(tmp_path: Path) -> None:
    """AC1: one altered byte of the model makes `--check` non-zero, naming that file."""
    regen = regen_module()
    for name in regen.ARTIFACT_FILENAMES:
        shutil.copyfile(FIXTURE_DIR / name, tmp_path / name)

    # The copies agree with the committed set, so the check is measuring the mutation
    # below and not a stale copy.
    assert regen.diverging_artifacts(tmp_path, FIXTURE_DIR) == ()

    altered = tmp_path / regen.MODEL_FILENAME
    data = bytearray(altered.read_bytes())
    data[0] = data[0] ^ 0x20
    altered.write_bytes(bytes(data))

    diverging = regen.diverging_artifacts(tmp_path, FIXTURE_DIR)
    assert diverging == (regen.MODEL_FILENAME,)

    result = regen.CheckResult(
        regenerated=FIXTURE_DIR, meta=fixture_meta(), pinned=(), artifacts=diverging
    )
    assert result.exit_code == 1
    assert any(regen.MODEL_FILENAME in line for line in result.report())

    # An artifact that is not committed at all counts as diverging: a `--check` that
    # passed on a tree with no fixture model would be worse than no check.
    assert regen.diverging_artifacts(tmp_path / "absent", FIXTURE_DIR) == regen.ARTIFACT_FILENAMES


def test_promote_refuses_a_set_that_is_not_one_artifact(tmp_path: Path) -> None:
    """`--from` copies a complete, self-consistent set and refuses anything else.

    This is the step that actually commits the artifacts — T-TRAIN-1 runs inside the
    `pipeline` container and can only reach the host through the `artifacts/` bind
    mount — so the failure it has to make impossible is a `tf.csv` from one run landing
    beside a model from another. Both halves of that are checked: all three files
    present, and the sidecar's SHA-256 being the model's.
    """
    regen = regen_module()
    source = tmp_path / "run"
    source.mkdir()

    with pytest.raises(FileNotFoundError, match=regen.MODEL_FILENAME):
        regen.promote(source, tmp_path / "destination")

    for name in regen.ARTIFACT_FILENAMES:
        shutil.copyfile(FIXTURE_DIR / name, source / name)
    (source / regen.TF_FILENAME).write_text("model_version\n", encoding="utf-8")
    meta = json.loads((source / regen.META_FILENAME).read_text(encoding="utf-8"))
    (source / regen.META_FILENAME).write_text(
        json.dumps({**meta, "sha256": "0" * 64}), encoding="utf-8"
    )

    with pytest.raises(ValueError, match="not one artifact set"):
        regen.promote(source, tmp_path / "destination")
    assert not (tmp_path / "destination").exists(), "a refused promotion wrote something"

    # And the committed set promotes onto itself unchanged, which is what the copy
    # after a green T-TRAIN-1 run does.
    destination = tmp_path / "committed"
    assert regen.promote(FIXTURE_DIR, destination) == tuple(
        destination / name for name in regen.ARTIFACT_FILENAMES
    )
    assert regen.diverging_artifacts(FIXTURE_DIR, destination) == ()


@pytest.mark.parametrize(
    "key",
    ["corpus_digest", "generator_seed", "scale", "model_version", "training", "splink_version"],
)
def test_pinned_input_divergence_names_the_input(key: str) -> None:
    """AC2: mutating any one pinned input is reported as that input, not as bytes.

    Parametrized over all six of S8.3's inputs rather than one representative: the
    failure this guards against is an input that is *listed* in the sidecar and never
    compared, which a single-case test would not distinguish from a compared one.
    """
    regen = regen_module()
    committed = fixture_meta()
    assert set(regen.PINNED_INPUT_KEYS) == {
        "corpus_digest",
        "generator_seed",
        "scale",
        "model_version",
        "training",
        "splink_version",
    }
    assert regen.diverging_pinned_inputs(committed, committed) == ()

    mutated = {**committed, key: "a value the committed sidecar does not carry"}
    diverging = regen.diverging_pinned_inputs(committed, mutated)

    assert len(diverging) == 1, diverging
    assert diverging[0].startswith(f"{key}:"), diverging


def test_reproducibility_test_is_slow_marked_and_registered() -> None:
    """AC7: `-m "not slow"` collects zero tests from the T-TRAIN-1 module.

    Two arms, because "collects nothing" is also true of a module that fails to import
    or that holds no tests at all — the unmarked run is what shows there was something
    there to deselect.
    """
    markers = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))["tool"]["pytest"]["ini_options"][
        "markers"
    ]
    assert any(marker.startswith("slow:") for marker in markers), markers

    def collect(*flags: str) -> list[str]:
        completed = subprocess.run(
            [
                sys.executable,
                "-m",
                "pytest",
                "--collect-only",
                "-q",
                "-p",
                "no:cacheprovider",
                *flags,
                str(REPRODUCIBILITY_TEST),
            ],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        return [
            line for line in completed.stdout.splitlines() if line.startswith("tests/integration/")
        ]

    assert collect(), "the T-TRAIN-1 module collects nothing even unfiltered"
    assert collect("-m", "not slow") == []


def test_ci_integration_job_excludes_slow() -> None:
    """AC7: the CI integration job's pytest invocation carries `-m "not slow"`.

    M4's exit criterion is a full PR path under ten minutes, enforced by the sum of the
    job timeouts. One EM fit over ten thousand records does not fit inside it, which is
    why the exclusion is in the workflow rather than left to whoever dispatches it.
    """
    workflow = yaml.safe_load(CI_WORKFLOW.read_text(encoding="utf-8"))
    steps = workflow["jobs"]["integration"]["steps"]
    suite = [step for step in steps if "pytest tests/integration" in step.get("run", "")]

    assert len(suite) == 1, [step.get("name") for step in steps]
    assert '-m "not slow"' in suite[0]["run"], suite[0]["run"]
