"""Regenerate `fixtures/static/model_test_v1.*` — the model scenario tests load.

S4.3.2 item 6 and S12 both state the reason this artifact is committed rather than
fitted per run: **scenario tests never train.** EM over `base_10`'s 23 records is
degenerate — the u estimate draws from at most 253 pairs and m from at most 18 — so
the model such a training produces is noise. The model is instead fitted **once**
against the S10.1 generator's corpus at the S10.2 `10k` scale, under
`configs/test.yaml`'s `generator.seed`, and committed as three files:

* `model_test_v1.json` — the fitted Splink settings document;
* `model_test_v1.tf.csv` — the frozen `tf_lookup` rows it was fitted alongside. They
  are part of the artifact set and not an optimisation: a scenario test that loaded
  the settings without them would have Splink compute term frequency from `base_10`,
  and INV-SCORE (S4.3.3, D4) would be false for every pair it scored;
* `model_test_v1.meta.json` — the sidecar pinning the six inputs T-TRAIN-1 (S8.3)
  requires to be identical for byte equality, plus the `config_hash`, the DuckDB pin
  and the SHA-256 of the settings document.

**Byte equality is a claim about fully pinned inputs**, which is why S8.3 names the
corpus in the T-TRAIN-1 row rather than leaving it to the test author, and why this
module writes the scale into the sidecar as a literal `(personas=4000,
records=10000)` pair: `benchmarks/scales.yaml` (S10.2) does not exist until M5, and a
generator reading it would make this the module M5 has to change.

Two determinism decisions are load-bearing and easy to lose:

* The `model_version` and the `tf_snapshot_id` are **pinned literals**, not the
  `max+1` allocation of S4.3.2 and not a freshly minted ULID. Both appear in the
  committed `tf.csv` and in the sidecar, so a value that moved between two
  regenerations would diff every row of the frozen TF table.
* The training corpus is materialized `ORDER BY record_key`.
  `estimate_u_using_random_sampling` samples the corpus relation, and a seeded sample
  over an unordered scan is reproducible only for as long as the scan order is — which
  is a property of how DuckLake happened to write the Parquet files, not a property
  anything in this repository pins.
* The fit runs **single-threaded**. EM's m estimates are `DOUBLE` sums over the
  corpus, floating-point addition is not associative, and DuckDB's parallel
  aggregation reduces the per-thread partial sums in whatever order the threads
  finish in. Two fits over one corpus therefore agree to about fifteen significant
  figures and differ in the last two or three — invisible in every score the model
  produces, and fatal to a byte comparison. One thread is one reduction order. The u
  estimates and the frozen TF values are unaffected either way, which is why the
  narrowing is applied to the fit rather than to the whole regeneration.

Usage::

    python scripts/regen_fixture_model.py                     # write the three artifacts
    python scripts/regen_fixture_model.py --check             # regenerate and diff instead
    python scripts/regen_fixture_model.py --from artifacts/fixture_model   # promote a run's output

The first two forms need the S7 substrate: regeneration ingests and standardizes into a
**disposable** lake namespace of its own, which it mints and reclaims per S8.1 step 4,
so it never touches the namespace of whatever else is running. The third does not, and
is how the artifacts actually get committed: T-TRAIN-1 runs inside the `pipeline`
container, whose only writable path onto the host is the `artifacts/` bind mount
(S7.1), so it leaves what it regenerated there and `--from` copies that set over
`fixtures/static/` — after checking the sidecar really describes the model beside it,
because a half-copied set is three files that were never one artifact.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Iterator, Mapping, Sequence
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any, Final

import duckdb
import splink
from ulid import ULID

from er.config.hashing import config_hash
from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, render_dbt_vars, run_dbt
from er.lake.catalog import catalog_connect, drop_metadata_schema
from er.lake.columns import STD_RECORD_COLUMNS
from er.lake.ducklake import LAKE_ALIAS, attach_statements, connect, detach
from er.lake.init import init_lake
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.objectstore import ObjectStore
from er.matching.tf import STD_RECORDS_RELATION, TF_LOOKUP_RELATION
from er.matching.train import TRAIN_CORPUS_RELATION, train_model

__all__ = [
    "ARTIFACT_FILENAMES",
    "FIXTURE_DIR",
    "FIXTURE_MODEL_VERSION",
    "FIXTURE_STEM",
    "FIXTURE_TF_SNAPSHOT_ID",
    "META_FILENAME",
    "MODEL_FILENAME",
    "PINNED_INPUT_KEYS",
    "SCALE",
    "TF_CSV_HEADER",
    "TF_FILENAME",
    "CheckResult",
    "RegenResult",
    "build_parser",
    "check",
    "corpus_digest",
    "diverging_artifacts",
    "diverging_pinned_inputs",
    "main",
    "pinned_inputs",
    "regenerate",
]

REPO_ROOT: Final = Path(__file__).resolve().parents[1]

#: Where the three artifacts live, and the stem all three share. `fixtures/static/`
#: is the committed-data directory of S3; the stem is `model_test_v1` because S4.3.2
#: item 6 and S12 both name `fixtures/static/model_test_v1.json` literally.
FIXTURE_DIR: Final = REPO_ROOT / "fixtures" / "static"
FIXTURE_STEM: Final = "model_test_v1"
MODEL_FILENAME: Final = f"{FIXTURE_STEM}.json"
TF_FILENAME: Final = f"{FIXTURE_STEM}.tf.csv"
META_FILENAME: Final = f"{FIXTURE_STEM}.meta.json"

#: The artifact set, in the order a reader meets it: the model, its frozen TF, and
#: the sidecar that pins what produced both.
ARTIFACT_FILENAMES: Final[tuple[str, ...]] = (MODEL_FILENAME, TF_FILENAME, META_FILENAME)

#: The S10.2 `10k` row, written literally. `benchmarks/scales.yaml` does not exist
#: until M5 and this artifact is an M3 deliverable, so the pair is stated here and
#: pinned into the sidecar rather than read from a file that is not there yet.
SCALE: Final[Mapping[str, Any]] = {"name": "10k", "personas": 4000, "records": 10000}

#: The version the committed artifact carries. Pinned rather than allocated: S4.3.2
#: allocates `max+1` against a live registry, and a fixture whose version depended on
#: what a lake happened to hold would rewrite `tf.csv` on every regeneration.
FIXTURE_MODEL_VERSION: Final = "v0001"

#: The TF snapshot the frozen rows are keyed by. A pinned literal for the same
#: reason, and ULID-shaped so it sorts and reads like every other `tf_snapshot_id`
#: (D10) — `new_tf_snapshot_id()` mints from the clock and cannot be committed.
FIXTURE_TF_SNAPSHOT_ID: Final = "01JWMDTFSNAP00000000000001"

#: `model_test_v1.tf.csv`'s header, in `tf_lookup` column order (S5).
TF_CSV_HEADER: Final[tuple[str, ...]] = (
    "model_version",
    "tf_snapshot_id",
    "column_name",
    "value",
    "tf_value",
)

#: The six inputs S8.3 requires to be identical for the bytes to match, as sidecar
#: keys. T-TRAIN-1 compares these BEFORE it compares bytes, so a failure names the
#: input that diverged instead of reporting that two documents differ.
PINNED_INPUT_KEYS: Final[tuple[str, ...]] = (
    "corpus_digest",
    "generator_seed",
    "scale",
    "model_version",
    "training",
    "splink_version",
)

#: The dbt selections `er standardize` runs, in order (S4.2). `seed` precedes both:
#: `name_variants` reaches `nickname_variants` through `ref()`.
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

#: S8.1 step 2's pair, for the namespace this script mints for itself. The prefix is
#: `er_test_` so that `er lake reset` and the harness teardown both still recognise a
#: namespace this script leaked, and the `regen_` infix says which tool leaked it.
_METADATA_SCHEMA_PREFIX: Final = "er_test_regen_"
_DATA_PATH_TEMPLATE: Final = "s3://lake/test/regen_{ns}/"

#: S8.1 step 4's first two statements, in the spelling `duckdb==1.5.5`'s ducklake
#: extension accepts (see `tests/conftest.py`, which documents the divergence).
_EXPIRE_SNAPSHOTS: Final = f"CALL ducklake_expire_snapshots('{LAKE_ALIAS}', older_than => now())"
_CLEANUP_OLD_FILES: Final = f"CALL ducklake_cleanup_old_files('{LAKE_ALIAS}', cleanup_all => true)"

#: The environment variable :func:`~er.matching.api.splink_api` pins the Splink
#: session's DuckDB thread count from (S7.1), and what this script pins it to while it
#: fits. See the module docstring: a parallel `DOUBLE` reduction has no fixed order,
#: and byte equality of the fitted m values needs one.
_THREADS_ENV: Final = "ER_DUCKDB_THREADS"
_TRAINING_THREADS: Final = "1"

_TF_LOOKUP: Final = f"{SCHEMA_QUALIFIER}.{TF_LOOKUP_RELATION}"
_STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.{STD_RECORDS_RELATION}"

#: The frozen rows, read back in the order they are committed in. Ordering by
#: `(column_name, value)` rather than trusting the insert order: the rows are written
#: by one `INSERT … UNION ALL` whose branch order is config order, and a committed
#: file whose sort key was an implementation detail of that statement would diff the
#: first time the statement was rewritten.
_SELECT_TF_SQL: Final = f"""
SELECT column_name, value, tf_value FROM {_TF_LOOKUP}
 WHERE model_version = ? AND tf_snapshot_id = ?
 ORDER BY column_name, value
"""

#: The training corpus, materialized local and bare for `build_training_linker`, and
#: **ordered**: the u estimate samples this relation under a seed, so its row order is
#: one of the inputs byte equality depends on.
_CORPUS_SQL: Final = (
    f"CREATE OR REPLACE TABLE {TRAIN_CORPUS_RELATION} AS "
    f"SELECT {', '.join(STD_RECORD_COLUMNS)} FROM {_STD_RECORDS} ORDER BY record_key"
)


def _config_path() -> Path:
    """The S6 document this regeneration reads.

    `ER_CONFIG` when Compose set it (S7.1), and `configs/test.yaml` otherwise —
    which is the same file, and is what S6 calls "the file the fixtures and CI use
    verbatim".
    """
    supplied = os.environ.get("ER_CONFIG")
    return Path(supplied) if supplied else REPO_ROOT / "configs" / "test.yaml"


def corpus_digest(paths: Sequence[Path]) -> str:
    """A SHA-256 over the emitted corpus, name and bytes, in name order.

    The name is hashed alongside the bytes so that two files swapping contents is a
    different digest, and the order is the file name's rather than emission order so
    that the digest is a property of the corpus rather than of the loop that wrote it.
    """
    digest = hashlib.sha256()
    for path in sorted(paths, key=lambda entry: entry.name):
        digest.update(path.name.encode("utf-8"))
        digest.update(b"\x00")
        digest.update(path.read_bytes())
    return digest.hexdigest()


def pinned_inputs(cfg: Config, digest: str) -> dict[str, Any]:
    """The six inputs of S8.3's T-TRAIN-1 row, as they are written to the sidecar.

    Args:
        cfg: the validated S6 document. `training:` is taken as `model_dump()` and not
            as the fields the estimators happen to read — S4.3.2 persists the block
            verbatim, so a key no estimator consumes still has to be pinned.
        digest: :func:`corpus_digest` over the corpus the model was fitted on.
    """
    return {
        "corpus_digest": digest,
        "generator_seed": cfg.generator.seed,
        "scale": dict(SCALE),
        "model_version": FIXTURE_MODEL_VERSION,
        "training": cfg.training.model_dump(),
        "splink_version": splink.__version__,
    }


def diverging_pinned_inputs(
    committed: Mapping[str, Any], regenerated: Mapping[str, Any]
) -> tuple[str, ...]:
    """Which of the six pinned inputs disagree, named, in :data:`PINNED_INPUT_KEYS` order.

    Returned as rendered lines rather than as bare key names because the whole point
    of asserting the inputs before the bytes is that the failure says *what* moved:
    "generator_seed: committed 42, regenerated 43" is actionable and "bytes differ"
    is not.
    """
    diverging: list[str] = []
    for key in PINNED_INPUT_KEYS:
        if key not in committed:
            diverging.append(f"{key}: absent from the committed {META_FILENAME}")
            continue
        if committed[key] != regenerated.get(key):
            diverging.append(
                f"{key}: committed {json.dumps(committed[key], sort_keys=True)}, "
                f"regenerated {json.dumps(regenerated.get(key), sort_keys=True)}"
            )
    return tuple(diverging)


def diverging_artifacts(committed_dir: Path, regenerated_dir: Path) -> tuple[str, ...]:
    """Which of the three artifacts differ byte for byte, in :data:`ARTIFACT_FILENAMES` order.

    A file that is not committed at all counts as diverging: the first regeneration
    of a fresh checkout has nothing to compare against, and reporting that as agreement
    would make `--check` pass on a tree carrying no fixture model.
    """
    diverging: list[str] = []
    for name in ARTIFACT_FILENAMES:
        committed = committed_dir / name
        regenerated = regenerated_dir / name
        if not committed.is_file():
            diverging.append(name)
            continue
        if not regenerated.is_file() or committed.read_bytes() != regenerated.read_bytes():
            diverging.append(name)
    return tuple(diverging)


@contextmanager
def _disposable_namespace() -> Iterator[duckdb.DuckDBPyConnection]:
    """Mint a lake namespace, `er init` it, yield it, and reclaim it (S8.1).

    The namespace is exactly the pair `(ER_LAKE_METADATA_SCHEMA, ER_LAKE_DATA_PATH)`
    and nothing else in the S4.0b attach sequence varies with it (S7.2), so the
    credentials Compose supplied are deliberately left alone. Both variables are
    exported into `os.environ` rather than merely passed around: `er ingest` runs as a
    subprocess and dbt runs as a subprocess, and neither would otherwise reach this
    namespace.

    Reclaim order is S8.1 step 4's and is not merely tidy — snapshots are expired and
    unreferenced files cleaned up *before* the prefix is deleted, and the lake is
    detached before the catalog schema is dropped. `ExitStack` runs every step even if
    one fails, because a partial teardown leaks an orphan Parquet prefix and a catalog
    schema no later `DROP` will name.
    """
    ns = str(ULID()).lower()
    metadata_schema = f"{_METADATA_SCHEMA_PREFIX}{ns}"
    data_path = _DATA_PATH_TEMPLATE.format(ns=ns)
    previous = {
        name: os.environ.get(name)
        for name in ("ER_LAKE_METADATA_SCHEMA", "ER_LAKE_DATA_PATH", "ER_LAKE_ALIAS")
    }
    os.environ["ER_LAKE_METADATA_SCHEMA"] = metadata_schema
    os.environ["ER_LAKE_DATA_PATH"] = data_path
    os.environ["ER_LAKE_ALIAS"] = LAKE_ALIAS
    try:
        store = ObjectStore.from_env()
        with catalog_connect() as catalog, connect() as connection:
            with ExitStack() as reclaim:
                reclaim.callback(drop_metadata_schema, catalog, metadata_schema)
                reclaim.callback(detach, connection)
                reclaim.callback(store.delete_prefix, data_path)
                reclaim.callback(connection.execute, _CLEANUP_OLD_FILES)
                reclaim.callback(connection.execute, _EXPIRE_SNAPSHOTS)
                init_lake(connection=connection)
                yield connection
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


@contextmanager
def _single_threaded(connection: duckdb.DuckDBPyConnection) -> Iterator[None]:
    """Pin this connection and the Splink session it opens to one DuckDB thread.

    Both, and not just the connection: :func:`~er.matching.api.splink_api` issues its
    own `SET threads` from :data:`_THREADS_ENV` when the linker is built, so a setting
    made only on the connection would be overwritten a moment later by the value
    Compose supplied.

    The variable is restored on the way out. The connection's own setting is not —
    everything this script does with it afterwards is teardown, and a regeneration
    that reset it would be claiming a thread count for statements whose results
    nothing reads.
    """
    previous = os.environ.get(_THREADS_ENV)
    os.environ[_THREADS_ENV] = _TRAINING_THREADS
    connection.execute(f"SET threads = {_TRAINING_THREADS}")
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop(_THREADS_ENV, None)
        else:
            os.environ[_THREADS_ENV] = previous


def _generator(module: str) -> ModuleType:
    """Import a `fixtures/generator` module, whose parent is not on the path.

    `fixtures/` is committed data rather than a distribution (S3), so nothing installs
    it, and a bare `sys.path` statement followed by an import is an E402 no ticket may
    suppress. `tests/unit/generator/` reaches the same modules the same way.
    """
    entry = str(REPO_ROOT / "fixtures")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(f"generator.{module}", fromlist=["_"])


def _generate_corpus(cfg: Config, out_dir: Path) -> tuple[list[Path], str]:
    """Emit the S10.2 `10k` corpus at `generator.seed` and digest what was written."""
    emit = _generator("emit")
    spec = emit.CorpusSpec(
        seed=cfg.generator.seed,
        personas=int(SCALE["personas"]),
        records=int(SCALE["records"]),
    )
    personas = _generator("personas").generate_personas(
        spec.seed, spec.personas, spec.household_rate
    )
    written: list[Path] = emit.emit_corpus(spec, personas, out_dir, config=cfg)
    return written, corpus_digest(written)


def _deliver(corpus_dir: Path, drop_root: Path, cfg: Config) -> Path:
    """Lay the emitted CSVs out as the drop-folder root `er ingest --path` reads.

    The generator writes `<out>/<source>.csv` (S10.1) while S4.1 reads
    `<path>/<source>/`, so the files are copied into the shape the adapter discovers
    rather than the adapter being taught a second layout.
    """
    for source in cfg.sources:
        directory = drop_root / source
        directory.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(corpus_dir / f"{source}.csv", directory / f"{source}.csv")
    return drop_root


def _ingest(cfg: Config, drop_root: Path, config_path: Path) -> None:
    """Run `er ingest` once per source, as the pipeline does (S4.1).

    The installed console script rather than :func:`~er.ingest.landing.ingest_delivery`
    directly: the artifact this produces has to be the artifact the pipeline would
    produce, and the stage does more than the landing function — it writes the `runs`
    and `ingest_batches` rows the corpus snapshot is later read against.
    """
    for source in cfg.sources:
        completed = subprocess.run(
            [
                "er",
                "ingest",
                "--source",
                source,
                "--path",
                str(drop_root),
                "--config",
                str(config_path),
            ],
            capture_output=True,
            text=True,
            env=dict(os.environ),
            check=False,
        )
        if completed.returncode != 0:
            raise RuntimeError(
                f"er ingest --source {source} exited {completed.returncode}:\n"
                f"{completed.stdout}{completed.stderr}"
            )


def _standardize(connection: duckdb.DuckDBPyConnection, cfg: Config, artifacts: Path) -> None:
    """Build the dbt seed, staging and intermediate models onto the corpus (S4.2).

    Driven through :func:`~er.dbt_runner.run_dbt` with the Python connection detached
    for the duration, which is S4.0b's rule: dbt opens its own connection to the same
    lake, and two writers against one DuckLake attachment is the failure that rule
    exists to prevent.
    """
    if not (Path(DBT_PROJECT_DIR) / "dbt_packages" / "dbt_utils").is_dir():
        completed = subprocess.run(
            ["dbt", "deps", "--project-dir", DBT_PROJECT_DIR, "--profiles-dir", DBT_PROFILES_DIR],
            capture_output=True,
            text=True,
            check=False,
        )
        if completed.returncode != 0:
            raise RuntimeError(f"dbt deps exited {completed.returncode}:\n{completed.stdout}")

    def reattach() -> None:
        for statement in attach_statements():
            connection.execute(statement)

    run_id = str(ULID())
    for command, select in (
        ("seed", None),
        ("build", STAGING_SELECTOR),
        ("build", INTERMEDIATE_SELECTOR),
    ):
        run_dbt(
            command,
            select=select,
            vars=render_dbt_vars(cfg, run_id),
            target="lake",
            close_conn=lambda: detach(connection),
            reopen_conn=reattach,
            project_dir=DBT_PROJECT_DIR,
            profiles_dir=DBT_PROFILES_DIR,
            artifacts_dir=artifacts,
        )


def _tf_rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str, str, str, float]]:
    """The frozen rows of this regeneration's TF key, in committed order."""
    return [
        (
            FIXTURE_MODEL_VERSION,
            FIXTURE_TF_SNAPSHOT_ID,
            str(column_name),
            str(value),
            float(tf_value),
        )
        for column_name, value, tf_value in connection.execute(
            _SELECT_TF_SQL, [FIXTURE_MODEL_VERSION, FIXTURE_TF_SNAPSHOT_ID]
        ).fetchall()
    ]


def _write_tf_csv(path: Path, rows: Sequence[tuple[str, str, str, str, float]]) -> None:
    """Write the frozen rows as `model_test_v1.tf.csv`.

    `lineterminator='\\n'` because the file is committed and compared byte for byte,
    and `repr` for `tf_value` because Python's shortest round-tripping float repr is
    the one rendering that reads the same value back.
    """
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle, lineterminator="\n")
        writer.writerow(TF_CSV_HEADER)
        for model_version, tf_snapshot_id, column_name, value, tf_value in rows:
            writer.writerow([model_version, tf_snapshot_id, column_name, value, repr(tf_value)])


@dataclass(frozen=True)
class RegenResult:
    """One regeneration: where it wrote, and the sidecar it wrote there."""

    out_dir: Path
    meta: Mapping[str, Any]

    @property
    def model_path(self) -> Path:
        """The regenerated settings document."""
        return self.out_dir / MODEL_FILENAME


def regenerate(out_dir: Path, *, cfg: Config | None = None) -> RegenResult:
    """Fit the fixture model from scratch and write the three artifacts into ``out_dir``.

    The order is the pipeline's: generate the corpus, ingest it, standardize it, then
    fit — every step of it inside a lake namespace this call mints and reclaims, so a
    regeneration run beside a test session cannot see or be seen by it.

    Args:
        out_dir: where the three artifacts are written; created if absent.
        cfg: the validated S6 document; loaded from `ER_CONFIG` (or `configs/test.yaml`)
            when omitted.

    Returns:
        The directory written and the sidecar document, which carries the six pinned
        inputs plus `config_hash`, `duckdb.__version__` and the model's SHA-256.

    Raises:
        RuntimeError: `er ingest` or `dbt deps` exited non-zero.
        er.errors.StageFailure: the fitted settings could not be read back (S4.3.2).
    """
    config_path = _config_path()
    resolved = load_config(config_path) if cfg is None else cfg
    out_dir.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="er-regen-") as scratch:
        workdir = Path(scratch)
        corpus_dir = workdir / "corpus"
        _, digest = _generate_corpus(resolved, corpus_dir)

        with _disposable_namespace() as connection:
            _ingest(resolved, _deliver(corpus_dir, workdir / "drop", resolved), config_path)
            _standardize(connection, resolved, workdir / "artifacts")

            with _single_threaded(connection):
                connection.execute(_CORPUS_SQL)
                result = train_model(
                    connection,
                    resolved,
                    TRAIN_CORPUS_RELATION,
                    model_version=FIXTURE_MODEL_VERSION,
                    tf_snapshot_id=FIXTURE_TF_SNAPSHOT_ID,
                )
                rows = _tf_rows(connection)

    model_bytes = (result.settings_json + "\n").encode("utf-8")
    (out_dir / MODEL_FILENAME).write_bytes(model_bytes)
    _write_tf_csv(out_dir / TF_FILENAME, rows)

    meta: dict[str, Any] = {
        **pinned_inputs(resolved, digest),
        "artifact": MODEL_FILENAME,
        "config_hash": config_hash(resolved),
        "duckdb_version": duckdb.__version__,
        "sha256": hashlib.sha256(model_bytes).hexdigest(),
        "tf_rows": len(rows),
        "tf_snapshot_id": FIXTURE_TF_SNAPSHOT_ID,
    }
    (out_dir / META_FILENAME).write_text(
        json.dumps(meta, sort_keys=True, indent=2) + "\n", encoding="utf-8"
    )
    return RegenResult(out_dir=out_dir, meta=meta)


@dataclass(frozen=True)
class CheckResult:
    """What one `--check` found: the pinned inputs first, then the bytes.

    The two lists are kept apart because they answer different questions. A `pinned`
    entry says the regeneration was not the same experiment — a different corpus, a
    different seed, a different Splink — and the byte comparison after it would only
    restate that in hex. An `artifacts` entry with no `pinned` entry is the
    interesting failure: the same declared inputs produced a different model.
    """

    regenerated: Path
    meta: Mapping[str, Any]
    pinned: tuple[str, ...]
    artifacts: tuple[str, ...]

    @property
    def diverging(self) -> tuple[str, ...]:
        """Every divergence, pinned inputs first."""
        return self.pinned + self.artifacts

    @property
    def exit_code(self) -> int:
        """What `--check` returns: ``0`` on agreement, ``1`` on any divergence."""
        return 1 if self.diverging else 0

    def report(self) -> tuple[str, ...]:
        """The lines `--check` prints, or an empty tuple when nothing diverged."""
        if not self.diverging:
            return ()
        lines = [f"{FIXTURE_STEM}: {len(self.diverging)} divergence(s)"]
        lines += [f"  pinned input {entry}" for entry in self.pinned]
        lines += [f"  artifact {entry} differs from the committed file" for entry in self.artifacts]
        lines.append(f"  the regenerated artifacts are at {self.regenerated}")
        return tuple(lines)


def check(
    out_dir: Path, *, committed_dir: Path = FIXTURE_DIR, cfg: Config | None = None
) -> CheckResult:
    """Regenerate into ``out_dir`` and diff against the committed artifacts.

    The pinned inputs are compared **before** the bytes, per S8.3's T-TRAIN-1 row: six
    things must be identical for byte equality to mean anything, and a run that names
    which of them moved is the difference between a diagnosis and a hex dump.
    """
    result = regenerate(out_dir, cfg=cfg)
    committed_meta_path = committed_dir / META_FILENAME
    committed_meta: Mapping[str, Any] = (
        json.loads(committed_meta_path.read_text(encoding="utf-8"))
        if committed_meta_path.is_file()
        else {}
    )
    return CheckResult(
        regenerated=out_dir,
        meta=result.meta,
        pinned=diverging_pinned_inputs(committed_meta, result.meta),
        artifacts=diverging_artifacts(committed_dir, out_dir),
    )


def promote(source: Path, destination: Path = FIXTURE_DIR) -> tuple[Path, ...]:
    """Copy a regenerated artifact set over the committed one.

    The check before the copy is the whole value of doing this with a command rather
    than by hand: the three files are one artifact, the sidecar says which model it
    describes, and copying two of three — or copying a `tf.csv` from one run beside a
    model from another — produces a tree that every cheap unit assertion passes and
    that T-TRAIN-1 fails minutes later for no legible reason.

    Args:
        source: a directory holding all three artifacts, e.g. the `artifacts/fixture_model/`
            a T-TRAIN-1 run leaves behind.
        destination: where they are committed; `fixtures/static/` unless overridden.

    Returns:
        The files written, in :data:`ARTIFACT_FILENAMES` order.

    Raises:
        FileNotFoundError: ``source`` does not hold all three artifacts.
        ValueError: the sidecar's `sha256` is not the model's, so the set was not
            produced by one regeneration.
    """
    missing = [name for name in ARTIFACT_FILENAMES if not (source / name).is_file()]
    if missing:
        raise FileNotFoundError(f"{source} holds no {', '.join(missing)}")

    meta = json.loads((source / META_FILENAME).read_text(encoding="utf-8"))
    digest = hashlib.sha256((source / MODEL_FILENAME).read_bytes()).hexdigest()
    if meta.get("sha256") != digest:
        raise ValueError(
            f"{source / META_FILENAME} pins sha256={meta.get('sha256')!r} but "
            f"{MODEL_FILENAME} hashes to {digest!r}; these three files are not one "
            f"artifact set and must not be committed together"
        )

    destination.mkdir(parents=True, exist_ok=True)
    for name in ARTIFACT_FILENAMES:
        shutil.copyfile(source / name, destination / name)
    return tuple(destination / name for name in ARTIFACT_FILENAMES)


def build_parser() -> argparse.ArgumentParser:
    """The command line: write the artifacts, or regenerate and diff them."""
    parser = argparse.ArgumentParser(
        prog="python scripts/regen_fixture_model.py",
        description=(
            "Regenerate fixtures/static/model_test_v1.{json,tf.csv,meta.json} from the "
            "S10.2 `10k` corpus at configs/test.yaml's generator.seed."
        ),
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--check",
        action="store_true",
        help="regenerate into a scratch directory and diff instead of writing",
    )
    mode.add_argument(
        "--from",
        dest="promote_from",
        type=Path,
        default=None,
        metavar="DIR",
        help="copy an already-regenerated set (e.g. artifacts/fixture_model) over --out",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help=(
            f"where to write; {FIXTURE_DIR.relative_to(REPO_ROOT)} when writing, a "
            f"temporary directory when --check"
        ),
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Regenerate the fixture model, or check it, and return the process exit code.

    Returns:
        ``0`` when the artifacts were written or promoted, or when `--check` found the
        committed artifacts and the regenerated ones identical; ``1`` when `--check`
        found any divergence, with every one of them named on stderr.
    """
    args = build_parser().parse_args(argv)
    if args.promote_from is not None:
        for path in promote(args.promote_from, FIXTURE_DIR if args.out is None else args.out):
            print(path)
        return 0

    if not args.check:
        written = regenerate(FIXTURE_DIR if args.out is None else args.out)
        for name in ARTIFACT_FILENAMES:
            print(written.out_dir / name)
        return 0

    with ExitStack() as stack:
        out_dir = args.out
        if out_dir is None:
            out_dir = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="er-check-")))
        result = check(out_dir)
        for line in result.report():
            print(line, file=sys.stderr)
        return result.exit_code


if __name__ == "__main__":
    raise SystemExit(main())
