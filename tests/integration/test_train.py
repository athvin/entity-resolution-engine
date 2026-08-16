"""`er train` against a real lake and a real object store (S4.3.2, S4.0, S5.2).

This suite is about the model artifact *lifecycle*, not about the model. What the
Splink call sequence does is ER-054's and is asserted against a spy there; what is
asserted here is everything S4.3.2 says has to be true of where the fitted model ends
up and of which one is live:

* the object is written before the registry row, so a failed upload leaves zero rows;
* `model_version` is zero-padded `max+1`, allocated inside the insert transaction;
* activating vN+1 supersedes vN in that same transaction, so "at most one active row"
  — a logical key with no constraint behind it (S5.0) — is never observably false;
* `params_path` names the tenant exactly once and the object at it round-trips;
* `corpus_snapshot` and `config_hash` are the S4 idempotency key `--if-changed` tests;
* the stage writes one `run_stages` row and exactly one stderr JSON line (S5.2);
* the frozen TF rows and the registry's `tf_tables_path` name the same key (S4.3.3).

**The corpus is built directly rather than through `er ingest` plus dbt.** That is a
deliberate narrowing, not a shortcut: the standardization path is asserted end to end
by `tests/integration/scenarios/`, nothing here depends on how a record was
standardized, and a dbt build per test would put ten minutes of unrelated work in
front of eight assertions about a registry. The corpus itself is ER-054's — twelve
personas, two records each, corrupted so that every comparison level is reachable —
because EM has to *converge* for any of this to be observable, and S4.3.2 warns that
EM over a scenario fixture is degenerate.

The lake, the catalog, the object store, the S3 round-trip and `er train` itself are
all real: the artifact goes to MinIO, the row goes to DuckLake through Postgres, and
every invocation below is the installed console script in this session's namespace.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from contextlib import contextmanager
from datetime import date, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
import yaml
from ulid import ULID

from er.config.hashing import config_hash
from er.config.loader import load_config
from er.config.schema import Config
from er.errors import ExitCode, PreconditionFailure, StageFailure, exit_code_for
from er.lake.ducklake import current_snapshot
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import (
    ACTIVE,
    SUPERSEDED,
    active_model,
    allocate_model_version,
    corpus_snapshot,
    load_model_settings,
    model_params_uri,
)
from er.lake.objectstore import ObjectStore
from er.matching.tf import STD_RECORDS_RELATION, parse_tf_tables_path
from er.matching.train import TRAIN_CORPUS_RELATION, run_train_stage, train_model

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
TF_LOOKUP: Final = f"{SCHEMA_QUALIFIER}.tf_lookup"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"

#: The S5 `int_std_records` column list and its types, verbatim from the dbt-owned
#: block. Written out rather than generated from `STD_RECORD_COLUMNS`, because the
#: types are half of what the corpus has to get right — `name_variants` is a
#: `LIST(VARCHAR)` and `birth_date` is a real `DATE`, which is what makes the
#: `variant_match` and `dob_same_year_month` levels reachable at all (S4.3.1).
STD_RECORDS_DDL: Final = f"""
CREATE TABLE {STD_RECORDS} (
    record_key        VARCHAR   NOT NULL,
    source_system     VARCHAR   NOT NULL,
    source_record_id  VARCHAR   NOT NULL,
    content_hash      VARCHAR   NOT NULL,
    std_version       VARCHAR   NOT NULL,
    given_name        VARCHAR,
    family_name       VARCHAR,
    name_variants     VARCHAR[] NOT NULL,
    email             VARCHAR,
    email_valid       BOOLEAN,
    phone_e164        VARCHAR,
    phone_valid       BOOLEAN,
    addr_number       VARCHAR,
    addr_street       VARCHAR,
    addr_unit         VARCHAR,
    addr_city         VARCHAR,
    addr_region       VARCHAR,
    addr_postal       VARCHAR,
    birth_date        DATE,
    updated_at_source TIMESTAMP,
    ingest_batch_id   VARCHAR   NOT NULL,
    ingested_at       TIMESTAMP NOT NULL
)
"""

#: Twelve personas, two records each. Two per persona is what gives EM anything to
#: estimate; the corruptions are what stop every comparison landing on its exact
#: level, which would leave every other level with a zero count.
PERSONA_COUNT: Final = 12

#: A fixed instant for the two `TIMESTAMP` columns. The corpus is compared with
#: itself across runs, so nothing in it may be a clock reading.
STAMP: Final = datetime(2026, 1, 1, 0, 0, 0)


def corpus_rows() -> list[tuple[Any, ...]]:
    """The frozen corpus, in `record_key` order and identical on every call."""
    rows: list[tuple[Any, ...]] = []
    for persona in range(PERSONA_COUNT):
        given = f"person{persona}"
        family = f"sur{persona}"
        email = f"p{persona}@example.com"
        phone = f"+1555000{persona:04d}"
        postal = f"{10000 + persona}"
        birth = date(1990, 1, persona % 27 + 1)
        for source, source_id, values in (
            ("crm", f"{persona:02d}", (given, email, phone, birth)),
            (
                "billing",
                f"{persona:02d}",
                (
                    given if persona % 3 else f"{given}x",
                    email if persona % 4 else f"{given}@other.example.com",
                    phone if persona % 5 else None,
                    birth if persona % 2 else date(1990, 1, (persona + 13) % 27 + 1),
                ),
            ),
        ):
            name, mail, telephone, dob = values
            rows.append(
                (
                    f"{source}:{source_id}",
                    source,
                    source_id,
                    f"{'0' * 63}{persona % 10}",
                    "1",
                    name,
                    family,
                    [given],
                    mail,
                    mail is not None,
                    telephone,
                    telephone is not None,
                    "1420",
                    "Judah Street",
                    None,
                    "San Francisco",
                    "CA",
                    postal,
                    dob,
                    STAMP,
                    "01JQZ8XKQ4T7VN3M2B9CDEFGHJ",
                    STAMP,
                )
            )
    return rows


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def registry_rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str]]:
    """Every `(model_version, status)` in the registry, in allocation order."""
    return [
        (str(version), str(status))
        for version, status in connection.execute(
            f"SELECT model_version, status FROM {MODEL_REGISTRY} ORDER BY model_version"
        ).fetchall()
    ]


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 stage records on ``stderr``, ignoring everything else on it.

    Splink and DuckDB both write to stderr, so "exactly one JSON line for the stage"
    is a claim about the lines that ARE stage records — which is why they are selected
    by the key set S5.2 gives them rather than by being parseable.
    """
    records: list[dict[str, Any]] = []
    for line in stderr.splitlines():
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(parsed, dict) and "stage" in parsed and "run_id" in parsed:
            records.append(parsed)
    return records


@contextmanager
def splink_on(connection: duckdb.DuckDBPyConnection) -> Iterator[None]:
    """Run Splink on the SESSION connection and put the search path back afterwards.

    :func:`~er.matching.api.splink_api` issues `SET schema`, and this suite shares the
    session connection with every other module (S8.1). Restoring it is what stops a
    later suite failing for a reason this one caused.
    """
    search_path = scalar(connection, "SELECT current_schema()")
    try:
        yield
    finally:
        connection.execute(f"SET schema = '{search_path}'")


class RefusingStore:
    """An object store whose upload fails, and which records that it was asked.

    The seam AC1 needs: "a registry row never points at a missing object" is only
    observable when the object cannot be written, and the failure has to come from the
    store so that everything before it in :func:`~er.matching.train.run_train_stage`
    runs exactly as it does in production.
    """

    def __init__(self) -> None:
        self.attempted: list[str] = []

    def put_bytes(self, uri: str, data: bytes) -> None:
        self.attempted.append(uri)
        raise StageFailure(f"refusing to upload {uri}")

    def get_bytes(self, uri: str) -> bytes:
        raise AssertionError(f"nothing was uploaded, so {uri} cannot be read")


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


@pytest.fixture
def corpus(initialised_lake: duckdb.DuckDBPyConnection) -> Iterator[duckdb.DuckDBPyConnection]:
    """`int_std_records` in the lake, holding the frozen corpus.

    Created here rather than by dbt: this suite asserts nothing about
    standardization, and `int_std_records` is dbt-owned, so `clean_lake` drops it
    between tests and every test starts from a corpus it wrote itself.
    """
    initialised_lake.execute(STD_RECORDS_DDL)
    rows = corpus_rows()
    placeholders = ",".join("?" for _ in rows[0])
    initialised_lake.executemany(f"INSERT INTO {STD_RECORDS} VALUES ({placeholders})", rows)
    yield initialised_lake


@pytest.fixture
def trained(corpus: duckdb.DuckDBPyConnection) -> duckdb.DuckDBPyConnection:
    """One successful `er train` over the corpus."""
    result = run_er("train")
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
    return corpus


def test_object_written_before_registry_row(corpus: duckdb.DuckDBPyConnection, cfg: Config) -> None:
    """AC1: a failed upload exits 1 and leaves the registry exactly as it was found.

    S4.3.2's "object first, registry row second" exists so that a `model_registry` row
    is a promise `er match` can cash. The reverse order fails silently: the row is
    written, the upload fails, and the next scoring run discovers it by fetching a key
    that is not there — long after the run that could have retried.
    """
    before = scalar(corpus, f"SELECT count(*) FROM {MODEL_REGISTRY}")
    assert before == 0, "the fixture lake starts with no registered model"

    store = RefusingStore()
    with splink_on(corpus), pytest.raises(StageFailure) as refusal:
        run_train_stage(
            corpus,
            cfg,
            store,
            run_id=str(ULID()),
            config_hash=config_hash(cfg),
        )

    # The refusal is the store's, so the upload really was attempted -- an
    # implementation that wrote the row first would also raise here, and only the
    # attempted URI distinguishes the two.
    assert store.attempted == [model_params_uri(cfg.storage.model_uri_prefix, "v0001")]
    assert exit_code_for(refusal.value) == int(ExitCode.STAGE_FAILURE) == 1
    assert scalar(corpus, f"SELECT count(*) FROM {MODEL_REGISTRY}") == before

    # And the version was never consumed: the next successful training gets it.
    assert allocate_model_version(corpus) == "v0001"


def test_model_version_allocation_is_zero_padded_max_plus_one(
    corpus: duckdb.DuckDBPyConnection,
) -> None:
    """AC2: three trainings allocate v0001, v0002, v0003, in that order.

    Zero-padded to a fixed width because `model_version` is a `VARCHAR` (S5) that
    `er match --model-version` and every join compare as text: `v10` sorting before
    `v2` would make "the latest model" a different row depending on who asked.
    """
    for expected in ("v0001", "v0002", "v0003"):
        result = run_er("train")
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        assert f"model_version={expected}" in result.stdout

    assert [version for version, _ in registry_rows(corpus)] == ["v0001", "v0002", "v0003"]


def test_activation_supersedes_previous_in_one_transaction(
    corpus: duckdb.DuckDBPyConnection,
) -> None:
    """AC2: after three trainings exactly one row is active, and it is the newest.

    DuckLake enforces no filtered uniqueness (S5.0), so nothing but the writer keeps
    "at most one active row" true — and `er match` loading "the" active row is
    undefined the moment there are two.
    """
    for _ in range(3):
        assert run_er("train").returncode == int(ExitCode.SUCCESS)

    assert registry_rows(corpus) == [
        ("v0001", SUPERSEDED),
        ("v0002", SUPERSEDED),
        ("v0003", ACTIVE),
    ]
    assert scalar(corpus, f"SELECT count(*) FROM {MODEL_REGISTRY} WHERE status = ?", ACTIVE) == 1
    assert active_model(corpus).model_version == "v0003"


def test_params_path_is_prefixed_once_and_round_trips(
    trained: duckdb.DuckDBPyConnection, cfg: Config, object_store: ObjectStore
) -> None:
    """AC3: the artifact URI is the prefix plus `model_v0001.json`, and it round-trips.

    The tenant appears exactly once because `storage.model_uri_prefix` already names
    it (V14, S6.1). Interpolating it again produces a URI that resolves and that
    nothing rejects — the object is simply somewhere no operator looks.
    """
    row = active_model(trained)
    prefix = cfg.storage.model_uri_prefix

    assert row.params_path.startswith("s3://")
    assert row.params_path.startswith(prefix)
    assert row.params_path.endswith("model_v0001.json")
    assert row.params_path.count(f"/{cfg.tenant}/") == 1, row.params_path

    fetched = load_model_settings(trained, object_store, row.model_version)
    # Non-circular: the same corpus and the same seeded `training:` block fit the same
    # document (ER-054 AC7), so re-fitting here and comparing is a claim about what was
    # uploaded rather than a re-read of it. The version is one this registry has not
    # allocated, so the TF rows it freezes cannot collide with the trained model's.
    trained.execute(
        f"CREATE OR REPLACE TABLE {TRAIN_CORPUS_RELATION} AS SELECT * FROM {STD_RECORDS}"
    )
    with splink_on(trained):
        refitted = train_model(trained, cfg, TRAIN_CORPUS_RELATION, model_version="v9999")

    assert fetched == refitted.settings
    assert fetched["comparisons"], "the uploaded artifact fitted no comparisons"


def test_corpus_snapshot_and_config_hash_recorded(
    trained: duckdb.DuckDBPyConnection, cfg: Config
) -> None:
    """AC4: the row records the corpus it was fitted on and the config it was fitted under.

    `corpus_snapshot` is the version `int_std_records` was last written at, and the
    check that it means what S5 says it means is time travel: reading the corpus at
    that version returns the corpus that was trained over. It is deliberately not the
    lake's current version — every `runs`, `run_stages` and `tf_lookup` write advances
    that, so the S4 idempotency key `(config_hash, corpus_snapshot)` would name a
    different corpus on every invocation and `--if-changed` could never fire.
    """
    row = active_model(trained)
    live = scalar(trained, f"SELECT count(*) FROM {STD_RECORDS}")

    assert row.corpus_snapshot == corpus_snapshot(trained, STD_RECORDS_RELATION)
    assert 0 < row.corpus_snapshot <= current_snapshot(trained)
    at_version = scalar(
        trained, f"SELECT count(*) FROM {STD_RECORDS} AT (VERSION => {row.corpus_snapshot})"
    )
    assert at_version == live == len(corpus_rows())

    assert row.config_hash == config_hash(cfg)

    # The whole `training:` block, verbatim: S4.3.2 persists it rather than the fields
    # the sequence happens to read, so a key no estimator consumes still survives into
    # `metrics` and an operator can see what the model was fitted under.
    document = yaml.safe_load((REPO_ROOT / "configs" / "test.yaml").read_text(encoding="utf-8"))
    assert row.metrics["training"] == document["training"]
    assert row.metrics["fitted_m_u"], "metrics carries no fitted m/u values"


def test_if_changed_exits_10_without_allocating(
    corpus: duckdb.DuckDBPyConnection, cfg: Config
) -> None:
    """AC5, AC8: `--if-changed` is a no-op on an unchanged key, and a no-model lake is exit 3."""
    # AC8: with nothing registered, the selection S4.3.2 step 4 makes has no answer,
    # and S4.0 gives "no status='active' model" exit 3.
    with pytest.raises(PreconditionFailure) as missing:
        active_model(corpus)
    assert exit_code_for(missing.value) == int(ExitCode.PRECONDITION) == 3

    # ... which is why `--if-changed` on a lake with no model trains rather than
    # refusing: there is no key to compare against.
    first = run_er("train", "--if-changed")
    assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr
    assert [version for version, _ in registry_rows(corpus)] == ["v0001"]

    skipped = run_er("train", "--if-changed")
    assert skipped.returncode == int(ExitCode.NOTHING_TO_DO), skipped.stdout + skipped.stderr
    assert [version for version, _ in registry_rows(corpus)] == ["v0001"]
    # The no-op still reports the model that made it unnecessary (S4.0's stdout column).
    assert "model_version=v0001" in skipped.stdout

    # The same invocation without the flag allocates: S4 makes a new version the
    # DEFAULT for `er train`, and `--if-changed` the exception.
    again = run_er("train")
    assert again.returncode == int(ExitCode.SUCCESS), again.stdout + again.stderr
    assert registry_rows(corpus) == [("v0001", SUPERSEDED), ("v0002", ACTIVE)]
    assert active_model(corpus).config_hash == config_hash(cfg)


def test_train_writes_run_stage_and_one_json_log_line(
    corpus: duckdb.DuckDBPyConnection,
) -> None:
    """AC6: one `run_stages` row, one stderr record, and the counters S4.3.2 identifies it by."""
    run_id = str(ULID())
    result = run_er("train", "--run-id", run_id)
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    rows = corpus.execute(
        f"SELECT stage, status, snapshot_start, snapshot_end, counters, rows_in "
        f"FROM {RUN_STAGES} WHERE run_id = ?",
        [run_id],
    ).fetchall()
    assert len(rows) == 1, rows
    stage, status, snapshot_start, snapshot_end, counters, rows_in = rows[0]

    assert stage == "train"
    assert status == "succeeded"
    assert snapshot_start is not None and snapshot_end is not None
    assert rows_in == len(corpus_rows())

    payload = json.loads(counters)
    row = active_model(corpus)
    assert payload["model_version"] == row.model_version
    assert payload["tf_snapshot_id"] == row.tf_snapshot_id
    assert payload["corpus_snapshot"] == row.corpus_snapshot

    # S5.2: the same record, as EXACTLY one JSON line on stderr. Splink writes to the
    # same stream, so the count is over the stage records rather than over the lines.
    records = stage_records(result.stderr)
    assert [record["stage"] for record in records] == ["train"]
    assert records[0]["run_id"] == run_id
    assert records[0]["model_version"] == row.model_version
    assert records[0]["tf_snapshot_id"] == row.tf_snapshot_id


def test_tf_lookup_and_tf_tables_path_agree(trained: duckdb.DuckDBPyConnection) -> None:
    """AC7: the registry's TF locator resolves to the rows `er train` froze (S4.3.3).

    `tf_tables_path` is a locator rather than a URI — the rows live in the lake — so
    the only thing that makes it useful is that parsing it returns the key that selects
    them. A locator naming a key with no rows is what
    :class:`~er.matching.tf.MissingTfLookupError` refuses a scoring run over.
    """
    row = active_model(trained)

    assert parse_tf_tables_path(row.tf_tables_path) == (row.model_version, row.tf_snapshot_id)
    frozen = scalar(
        trained,
        f"SELECT count(*) FROM {TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ?",
        row.model_version,
        row.tf_snapshot_id,
    )
    assert frozen > 0, "the registered model points at a TF key with no rows"

    # Every `tf: true` column contributed, which is what a scoring run registers; a
    # subset would leave Splink computing the rest from the corpus at hand (D4).
    materialized = {
        str(name)
        for (name,) in trained.execute(
            f"SELECT DISTINCT column_name FROM {TF_LOOKUP} "
            f"WHERE model_version = ? AND tf_snapshot_id = ?",
            [row.model_version, row.tf_snapshot_id],
        ).fetchall()
    }
    assert materialized == {"given_name", "family_name", "email"}
    assert row.metrics["tf_rows"] == frozen
