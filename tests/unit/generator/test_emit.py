"""The emitted corpus (DesignDoc.md S10.1, S8.2, S6, S3).

`emit.py` writes the files `er ingest` reads, so its output is a contract with three
separate authorities and each test below is graded against one of them rather than
against a restatement of the module:

* **`fixtures/static/base_10/base/*.csv`** owns the headers. The test reads the
  committed first lines and splits them; it never restates a header, because a header
  restated in two places is a header that drifts (M14). S6's
  `sources.<name>.columns` is the third corner of that pin and is checked here too.
* **A second PROCESS** owns the reproducibility claim. An in-process re-call proves
  almost nothing -- it shares the interpreter's hash seed and module state -- so both
  emissions run through `python -m fixtures.generator.cli` under a *different*
  `PYTHONHASHSEED`, which is what actually catches a `set` or `dict` iteration order
  leaking into the corpus (S10.1 requires byte equality across machines).
* **`configs/test.yaml`** owns the per-source `date_format`. The test reads the
  document rather than repeating `%m/%d/%Y`, so a config edit that forgets the
  fixture fails here instead of in `stg_*` thousands of rows later.

Nothing here opens a connection, spawns dbt or reads the lake. The subprocesses are
bare interpreters running the generator's own CLI.
"""

from __future__ import annotations

import csv
import os
import subprocess
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

from er.config.loader import load_config

REPO_ROOT = Path(__file__).resolve().parents[3]

#: S6 calls this "the file the fixtures and CI use verbatim".
TEST_CONFIG_PATH = REPO_ROOT / "configs" / "test.yaml"

#: The hand-authored fixture of S8.2, whose header rows are the authority for
#: `SOURCE_HEADERS`. Read only: it is a `protected_paths` entry of this ticket.
BASE_10_DIR = REPO_ROOT / "fixtures" / "static" / "base_10" / "base"

CONFIG = load_config(TEST_CONFIG_PATH)
SEED: int = CONFIG.generator.seed

#: Small enough that three subprocess emissions stay inside a unit-test budget, and
#: large enough that `records // personas` has a remainder -- 100 over 40 gives 20
#: personas three records and 20 personas two, so the uneven deal is exercised.
PERSONAS = 40
RECORDS = 100

#: A seed the corpus must NOT be invariant under (AC2).
OTHER_SEED = 43

CORPUS_FILES = ("crm.csv", "billing.csv", "webforms.csv", "truth.csv")


def _import(module: str) -> ModuleType:
    """Import a `fixtures/generator` module, whose parent is not on the path.

    `fixtures/` is committed data rather than a distribution, so nothing installs it,
    and a bare `sys.path` statement followed by an import is an E402 this ticket may
    not suppress.
    """
    entry = str(REPO_ROOT / "fixtures")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(f"generator.{module}", fromlist=["_"])


emit = _import("emit")
personas_module = _import("personas")


def _run_cli(out_dir: Path, seed: int, hash_seed: str) -> None:
    """Emit one corpus in a fresh interpreter under an explicit `PYTHONHASHSEED`."""
    environment = dict(os.environ, PYTHONHASHSEED=hash_seed)
    subprocess.run(
        [
            sys.executable,
            "-m",
            "fixtures.generator.cli",
            "--personas",
            str(PERSONAS),
            "--records",
            str(RECORDS),
            "--seed",
            str(seed),
            "--out",
            str(out_dir),
        ],
        cwd=REPO_ROOT,
        env=environment,
        check=True,
        capture_output=True,
    )


def _read_csv(path: Path) -> tuple[list[str], list[list[str]]]:
    """`(header, rows)` of one emitted file."""
    with path.open(encoding="utf-8", newline="") as handle:
        rows = list(csv.reader(handle))
    return rows[0], rows[1:]


@pytest.fixture(scope="module")
def corpus(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """One in-process emission, the subject of every claim that is not about bytes."""
    out_dir = tmp_path_factory.mktemp("corpus")
    spec = emit.CorpusSpec(seed=SEED, personas=PERSONAS, records=RECORDS)
    people = personas_module.generate_personas(spec.seed, spec.personas, spec.household_rate)
    emit.emit_corpus(spec, people, out_dir, config=CONFIG)
    return out_dir


def test_headers_match_base_10_literals() -> None:
    """AC1: `SOURCE_HEADERS` is the committed `base_10` header row, per source.

    Three corners are pinned to one another here: the committed fixture, the
    `SOURCE_HEADERS` literal, and the header S6's `sources.<name>.columns` implies.
    S8.2 makes the config the owner ("S6 owns the mapping and these headers are
    derived from it"), so all three are asserted rather than only the pair the
    generator happens to read.
    """
    for source in emit.SOURCE_ORDER:
        committed = (BASE_10_DIR / f"{source}.csv").read_text(encoding="utf-8").splitlines()[0]
        expected = tuple(committed.split(","))
        assert emit.SOURCE_HEADERS[source] == expected, (
            f"SOURCE_HEADERS[{source!r}] disagrees with the committed base_10 header row"
        )
        assert emit.source_header(CONFIG.sources[source]) == expected, (
            f"sources.{source}.columns in {TEST_CONFIG_PATH.name} no longer implies the "
            f"committed base_10 header row"
        )
        assert expected[-1] == "persona_id", "S8.2 makes persona_id the last field of every row"
        assert expected[0] == CONFIG.sources[source].record_id_column
        assert expected[-2] == CONFIG.sources[source].updated_at_column


def test_emission_is_byte_reproducible_across_processes(tmp_path: Path) -> None:
    """AC2: same seed, two processes, identical bytes; a different seed differs."""
    first, second, other = tmp_path / "a", tmp_path / "b", tmp_path / "c"
    _run_cli(first, SEED, "0")
    # A different PYTHONHASHSEED is the point: with the default randomisation two
    # runs could agree by luck, and this makes disagreement the expected outcome for
    # any generator that leaks a `set` or `dict` iteration order into the corpus.
    _run_cli(second, SEED, "12345")
    _run_cli(other, OTHER_SEED, "0")

    for name in CORPUS_FILES:
        assert (first / name).read_bytes() == (second / name).read_bytes(), (
            f"{name} differs between two processes at seed {SEED}"
        )

    assert any(
        (first / name).read_bytes() != (other / name).read_bytes() for name in CORPUS_FILES
    ), f"seed {OTHER_SEED} produced the same corpus as seed {SEED}; the seed is not being read"


def test_persona_id_is_last_column_and_truth_matches(corpus: Path) -> None:
    """AC3: `persona_id` is the last field, and `truth.csv` is exactly those triples."""
    emitted: set[tuple[str, str, str]] = set()
    for source in emit.SOURCE_ORDER:
        header, rows = _read_csv(corpus / f"{source}.csv")
        assert header == list(emit.SOURCE_HEADERS[source])
        for row in rows:
            assert len(row) == len(header), f"{source} row {row[0]!r} has the wrong field count"
            emitted.add((source, row[0], row[-1]))

    truth_header, truth_rows = _read_csv(corpus / emit.TRUTH_FILENAME)
    assert truth_header == list(emit.TRUTH_HEADER)
    labelled = {(row[1], row[2], row[0]) for row in truth_rows}

    assert len(truth_rows) == len(labelled), "truth.csv repeats a (source, record) key"
    assert labelled == emitted, "truth.csv and the source files disagree about the labels"


def test_row_and_persona_counts_match_spec(corpus: Path) -> None:
    """AC3: `--records` rows in total, `--personas` distinct labels, none unrepresented."""
    per_persona: Counter[str] = Counter()
    record_ids: set[str] = set()
    for source in emit.SOURCE_ORDER:
        _, rows = _read_csv(corpus / f"{source}.csv")
        for row in rows:
            per_persona[row[-1]] += 1
            record_ids.add(f"{source}|{row[0]}")

    assert sum(per_persona.values()) == RECORDS
    assert len(per_persona) == PERSONAS
    assert len(record_ids) == RECORDS, "a (source_system, source_record_id) key repeats"
    assert min(per_persona.values()) >= 1
    # 100 over 40 is an uneven deal; asserting the shape keeps a regression that
    # gives one persona 61 records and the rest one each from passing the two counts.
    assert set(per_persona.values()) == {RECORDS // PERSONAS, RECORDS // PERSONAS + 1}


def test_date_format_per_source(corpus: Path) -> None:
    """AC7: `birth_date` renders in the source's S6 `date_format`; missing is empty."""
    empties = 0
    for source in emit.SOURCE_ORDER:
        date_format = CONFIG.sources[source].date_format
        header, rows = _read_csv(corpus / f"{source}.csv")
        birth_date_index = header.index(CONFIG.sources[source].columns["birth_date"])
        for row in rows:
            value = row[birth_date_index]
            if not value:
                empties += 1
                continue
            assert value != "None", f"{source} wrote the literal None where a date is missing"
            # Round-trip rather than a regex: `strptime` accepting `04/01/1952` under
            # `%Y-%m-%d` is not something a shape check would catch, and re-rendering
            # proves the value is the format's own spelling of that date.
            parsed = datetime.strptime(value, date_format)
            assert parsed.strftime(date_format) == value
        assert any(row[birth_date_index] for row in rows), f"{source} emitted no date at all"

    # The three profiles all carry a `birth_date` missing rate, so an emission with no
    # empty cell would mean the axis never fired and the "never the literal None"
    # assertion above never ran.
    assert empties > 0, "no birth_date went missing; AC7's empty-field case is untested"


def test_date_formats_differ_across_sources() -> None:
    """S8.2: the two formats exist so `stg_*` date parsing is exercised, not assumed."""
    formats = {source: CONFIG.sources[source].date_format for source in emit.SOURCE_ORDER}
    assert formats["billing"] == "%m/%d/%Y"
    assert formats["crm"] == formats["webforms"] == "%Y-%m-%d"


def test_batch_delivery_continues_the_base_record_ids(tmp_path: Path) -> None:
    """S8.2.1's `batch` phase: an incremental delivery, disjoint keys, same personas.

    `--batch` is part of the CLI S10.2's scale table feeds, so the delivery it writes
    has to satisfy the one property the incremental path depends on: a batch record is
    a NEW record of an existing persona, never a re-issue of a base record's key.
    """
    batch_rows = 30
    spec = emit.CorpusSpec(seed=SEED, personas=PERSONAS, records=RECORDS, batch=batch_rows)
    people = personas_module.generate_personas(spec.seed, spec.personas, spec.household_rate)
    emit.emit_corpus(spec, people, tmp_path, config=CONFIG)

    batch_dir = tmp_path / emit.BATCH_DIRNAME
    base_keys: set[str] = set()
    batch_keys: set[str] = set()
    for source in emit.SOURCE_ORDER:
        _, base = _read_csv(tmp_path / f"{source}.csv")
        _, batch = _read_csv(batch_dir / f"{source}.csv")
        base_keys |= {f"{source}|{row[0]}" for row in base}
        batch_keys |= {f"{source}|{row[0]}" for row in batch}

    assert len(base_keys) == RECORDS
    assert len(batch_keys) == batch_rows
    assert not base_keys & batch_keys, "a batch record re-issued a base record's key"

    _, truth = _read_csv(batch_dir / emit.TRUTH_FILENAME)
    assert len(truth) == batch_rows
    assert {row[0] for row in truth} <= {person.persona_id for person in people}


def test_corpus_spec_rejects_a_shape_that_would_hide_a_persona() -> None:
    """`records < personas` would leave a ground-truth entity the pipeline never sees."""
    with pytest.raises(ValueError, match="records must be >= personas"):
        emit.CorpusSpec(seed=SEED, personas=10, records=9)
    with pytest.raises(ValueError, match="personas must be >= 1"):
        emit.CorpusSpec(seed=SEED, personas=0, records=0)
    with pytest.raises(ValueError, match="batch must be >= 0"):
        emit.CorpusSpec(seed=SEED, personas=2, records=2, batch=-1)


def test_emit_refuses_a_config_whose_columns_disagree_with_the_headers(tmp_path: Path) -> None:
    """M14: the headers are derived from `sources.<name>.columns` and never invented."""
    document = TEST_CONFIG_PATH.read_text(encoding="utf-8").replace(
        "given_name:   first_name", "given_name:   given_name", 1
    )
    drifted_path = tmp_path / "drifted.yaml"
    drifted_path.write_text(document, encoding="utf-8")
    drifted: Any = load_config(drifted_path)

    spec = emit.CorpusSpec(seed=SEED, personas=2, records=2)
    people = personas_module.generate_personas(spec.seed, spec.personas, spec.household_rate)
    with pytest.raises(ValueError, match="S6 owns the mapping"):
        emit.emit_corpus(spec, people, tmp_path / "out", config=drifted)
