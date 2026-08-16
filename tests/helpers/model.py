"""The committed fixture model, and the single way a scenario test acquires one.

S4.3.2 item 6 is one sentence — "scenario tests never train: they load the committed
`fixtures/static/model_test_v1.json`" — and this module is all of it. EM over
`base_10`'s 23 records is degenerate (S12: the u estimate draws from at most 253 pairs
and m from at most 18), so a scenario that trained would be scoring against noise and
every quality assertion in S8.3 would be measuring the fixture's arithmetic rather
than the pipeline.

:func:`load_fixture_model` therefore does what `er train` would have done, minus the
training: it inserts the frozen `tf_lookup` rows and one `status='active'`
`model_registry` row, and hands back the settings document. All three artifacts are
committed together and none of them is optional — a scenario that loaded the settings
without the frozen TF rows would have Splink compute term frequency from whatever
corpus was registered at predict time, which is exactly the drift D4 and INV-SCORE
(S4.3.3) exist to forbid.

Two things about the registry row this module writes are deliberately not what `er
train` writes, and both are visible in the values below rather than hidden:

* `params_path` names the **committed file on disk**, not an `s3://` object. Nothing
  uploaded it, so :func:`~er.lake.model_registry.load_model_settings` cannot fetch it;
  a scenario test uses the settings this function returns.
* `corpus_snapshot` is ``0``. The model was fitted against the S10.2 `10k` corpus in a
  namespace that no longer exists, and any other number here would be a claim about
  *this* lake's snapshot log that time travel would immediately falsify.

The version and the TF snapshot are spelled here as literals and asserted against the
sidecar by `tests/unit/fixtures/test_fixture_model.py`. Reading them out of the
sidecar at import time instead would make every module that imports this one fail to
even load on a tree whose fixture model has not been regenerated yet — including the
test whose whole job is to regenerate it.
"""

from __future__ import annotations

import csv
import json
from datetime import datetime
from pathlib import Path
from typing import Any, Final

import duckdb

from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.lake.model_registry import ACTIVE, SUPERSEDED
from er.matching.tf import TF_LOOKUP_RELATION, tf_tables_path

__all__ = [
    "FIXTURE_CORPUS_SNAPSHOT",
    "FIXTURE_META_PATH",
    "FIXTURE_MODEL_PATH",
    "FIXTURE_MODEL_VERSION",
    "FIXTURE_RUN_ID",
    "FIXTURE_STEM",
    "FIXTURE_TF_PATH",
    "FIXTURE_TF_SNAPSHOT_ID",
    "FIXTURE_TRAINED_AT",
    "fixture_meta",
    "fixture_settings",
    "fixture_tf_rows",
    "load_fixture_model",
]

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

#: The three committed artifacts, which share one stem (S4.3.2 item 6 names the first).
FIXTURE_STEM: Final = "model_test_v1"
_FIXTURE_DIR: Final = REPO_ROOT / "fixtures" / "static"
FIXTURE_MODEL_PATH: Final = _FIXTURE_DIR / f"{FIXTURE_STEM}.json"
FIXTURE_TF_PATH: Final = _FIXTURE_DIR / f"{FIXTURE_STEM}.tf.csv"
FIXTURE_META_PATH: Final = _FIXTURE_DIR / f"{FIXTURE_STEM}.meta.json"

#: The version and TF snapshot the committed artifact carries. Pinned by the sidecar
#: rather than by the `max+1` allocation of S4.3.2 (S8.3, T-TRAIN-1 input 4), and
#: restated here so importing this module never depends on the artifact existing.
FIXTURE_MODEL_VERSION: Final = "v0001"
FIXTURE_TF_SNAPSHOT_ID: Final = "01JWMDTFSNAP00000000000001"

#: The registry row's remaining scalars. All three are constants because a fixture
#: that carried a clock reading or a fresh ULID would make two loads of one committed
#: model two different rows, and `assert_ids_stable` compares across runs (S8.2.1).
FIXTURE_TRAINED_AT: Final = datetime(2026, 1, 1, 0, 0, 0)
FIXTURE_RUN_ID: Final = "01JWMDRN000000000000000000"
FIXTURE_CORPUS_SNAPSHOT: Final = 0

_TF_LOOKUP: Final = f"{SCHEMA_QUALIFIER}.{TF_LOOKUP_RELATION}"
_MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

#: Both column lists come off the S5 registry, in its order, for the reason
#: `er.lake.model_registry` reads them there: a relation that grows a column must not
#: leave a positional `INSERT` here quietly writing values into the wrong ones.
_TF_COLUMNS: Final[tuple[str, ...]] = REGISTRY[TF_LOOKUP_RELATION].column_names
_REGISTRY_COLUMNS: Final[tuple[str, ...]] = REGISTRY["model_registry"].column_names

_INSERT_TF_SQL: Final = (
    f"INSERT INTO {_TF_LOOKUP} ({', '.join(_TF_COLUMNS)}) "
    f"VALUES ({', '.join('?' for _ in _TF_COLUMNS)})"
)
_INSERT_REGISTRY_SQL: Final = (
    f"INSERT INTO {_MODEL_REGISTRY} ({', '.join(_REGISTRY_COLUMNS)}) "
    f"VALUES ({', '.join('?' for _ in _REGISTRY_COLUMNS)})"
)


def _read_json(path: Path) -> dict[str, Any]:
    """One committed JSON document, as a mapping.

    Raises:
        ValueError: the file holds something other than a JSON object.
    """
    document = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(document, dict):
        raise ValueError(f"{path.name} holds a {type(document).__name__}, not a JSON object")
    return dict(document)


def fixture_meta() -> dict[str, Any]:
    """The sidecar: the six pinned inputs, the `config_hash` and the model's SHA-256."""
    return _read_json(FIXTURE_META_PATH)


def fixture_settings() -> dict[str, Any]:
    """The committed Splink settings document, parsed."""
    return _read_json(FIXTURE_MODEL_PATH)


def fixture_tf_rows() -> tuple[tuple[str, str, str, str, float], ...]:
    """The committed frozen TF rows, in `tf_lookup` column order and file order.

    Raises:
        ValueError: the committed header is not `tf_lookup`'s column list. The file is
            read positionally below, so a reordered header would silently load
            `column_name` into `value`.
    """
    with FIXTURE_TF_PATH.open(encoding="utf-8", newline="") as handle:
        reader = csv.reader(handle)
        header = tuple(next(reader))
        if header != _TF_COLUMNS:
            raise ValueError(
                f"{FIXTURE_TF_PATH.name} header is {header}, expected {_TF_COLUMNS} "
                f"(the tf_lookup column list of S5)"
            )
        return tuple(
            (model_version, tf_snapshot_id, column_name, value, float(tf_value))
            for model_version, tf_snapshot_id, column_name, value, tf_value in reader
        )


def load_fixture_model(
    connection: duckdb.DuckDBPyConnection,
) -> tuple[str, str, dict[str, Any]]:
    """Install the committed model into ``connection``'s lake and return it.

    What it writes is exactly what a scoring run needs and nothing more: the frozen
    `tf_lookup` rows of one `(model_version, tf_snapshot_id)` key, and one
    `model_registry` row that `er match` will select as `status='active'` (S4.3.2 step
    4). Any previously active row is superseded in the process, because "at most one
    active row" is a logical key DuckLake enforces nothing about (S5.0) and a second
    active row makes the selection undefined.

    The insert is preceded by a delete of the same key rather than guarded by an
    upsert: a scenario test may call this twice inside one namespace, and appending
    would give every TF value two frequencies, which doubles the corpus the
    `register_term_frequency_lookup` join sees.

    Args:
        connection: a connection with the lake attached (S4.0b), whose `ddl.py`-owned
            relations already exist.

    Returns:
        ``(model_version, tf_snapshot_id, settings)`` — the two keys every scoring
        call needs and the settings document itself, which is how a caller obtains it
        without an object store to fetch `params_path` from.
    """
    settings = fixture_settings()
    rows = fixture_tf_rows()

    connection.execute(
        f"DELETE FROM {_TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ?",
        [FIXTURE_MODEL_VERSION, FIXTURE_TF_SNAPSHOT_ID],
    )
    connection.executemany(_INSERT_TF_SQL, [list(row) for row in rows])

    connection.execute(
        f"UPDATE {_MODEL_REGISTRY} SET status = ? WHERE status = ?", [SUPERSEDED, ACTIVE]
    )
    connection.execute(
        f"DELETE FROM {_MODEL_REGISTRY} WHERE model_version = ?", [FIXTURE_MODEL_VERSION]
    )
    meta = fixture_meta()
    # The row, positionally, in the S5 column order `_REGISTRY_COLUMNS` reads off the
    # registry: model_version, status, trained_at, corpus_snapshot, params_path,
    # tf_tables_path, tf_snapshot_id, config_hash, metrics, run_id. Positional rather
    # than a dict keyed by column name because `tests/unit/test_compare_helpers.py`
    # requires that no module here re-list a column name S5 already declares -- and
    # the length check below is what a relation that grew a column hits, instead of
    # every value silently shifting one place.
    #
    # `metrics` is the sidecar itself: S4.3.2 persists the verbatim `training:` block
    # there, and the sidecar carries that block because it is one of T-TRAIN-1's six
    # pinned inputs.
    row: tuple[Any, ...] = (
        FIXTURE_MODEL_VERSION,
        ACTIVE,
        FIXTURE_TRAINED_AT,
        FIXTURE_CORPUS_SNAPSHOT,
        str(FIXTURE_MODEL_PATH),
        tf_tables_path(FIXTURE_MODEL_VERSION, FIXTURE_TF_SNAPSHOT_ID),
        FIXTURE_TF_SNAPSHOT_ID,
        str(meta["config_hash"]),
        json.dumps(meta, sort_keys=True, ensure_ascii=False),
        FIXTURE_RUN_ID,
    )
    if len(row) != len(_REGISTRY_COLUMNS):
        raise ValueError(
            f"model_registry declares {len(_REGISTRY_COLUMNS)} columns and this fixture "
            f"supplies {len(row)}; the values above are positional (S5)"
        )
    connection.execute(_INSERT_REGISTRY_SQL, list(row))

    return FIXTURE_MODEL_VERSION, FIXTURE_TF_SNAPSHOT_ID, settings
