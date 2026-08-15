"""The ingest write's service-less half (DesignDoc.md S4.1, S4.0).

Three claims are properties of the *code* rather than of a lake, so they belong on
the S8.1 unit layer and are asserted here:

* an unknown ``--source`` exits ``2`` and does so **before any lake connection is
  opened**, which is checked by making the connection factory itself fail the test
  if it is ever called;
* the anti-join predicate is the S5.0 logical key of `raw_records`, not a second
  spelling of it that happens to agree today;
* ``er.ingest.landing.content_hash`` **is** ``er.ingest.hashing.content_hash`` —
  identity, not equality of output, because S4.1 pins the *cardinality* of the
  implementation and a second function that agrees on today's vectors would still
  be the defect it forbids.

Everything that needs rows in a lake — the append itself, the manifest row, the
counters — is in ``tests/integration/test_ingest_idempotence.py``.
"""

from __future__ import annotations

from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

from er import cli
from er.errors import ExitCode
from er.ingest import hashing, landing
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER

REPO_ROOT = Path(__file__).resolve().parents[2]

#: The S6 document the unit layer runs against (S8.1: no services).
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

runner = CliRunner()


def invoke(*args: str) -> Any:
    """Run the command tree with the repo's own S6 document."""
    return runner.invoke(cli.app, [*args, "--config", str(TEST_CONFIG)])


def test_unknown_source_exits_2_before_connecting(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """AC5: `--source nope` is exit 2, and no lake connection was opened.

    The guard is the point of the test: `er.cli.connect` is replaced with a factory
    that fails outright, so a refusal that happened *after* the `runs` row — or after
    the writer lock reached for the catalog — is a failure here rather than a pass
    with the right exit code.
    """

    def refuse() -> Any:
        raise AssertionError("S4.0: an unknown source must exit 2 before any connection")

    monkeypatch.setattr(cli, "connect", refuse)

    result = invoke("ingest", "--source", "nope", "--path", str(tmp_path))

    assert result.exit_code == int(ExitCode.CONFIG)
    assert "sources.nope" in result.stderr


def test_anti_join_key_is_source_system_source_record_id_content_hash() -> None:
    """AC1/AC2/AC3: the write's key is S5.0's logical key of `raw_records`.

    Both halves are asserted. The constant must equal S4.1's three columns, and the
    statement that does the write must join on all three of them and on nothing else
    — a predicate that dropped `content_hash` would turn version history into an
    upsert, and one that dropped `source_record_id` would append nothing after the
    first delivery.
    """
    assert landing.ANTI_JOIN_KEY == ("source_system", "source_record_id", "content_hash")
    assert landing.ANTI_JOIN_KEY == REGISTRY["raw_records"].keys[0].columns

    append = landing._APPEND
    for column in landing.ANTI_JOIN_KEY:
        assert f"r.{column} = d.{column}" in append, f"the anti-join ignores {column}"
    assert "NOT EXISTS" in append

    # D7: a corrected record ADDS a row, so no statement anywhere in the module may
    # rewrite or remove one. Matched against the statement KEYWORD plus the relation
    # rather than against the bare verb -- `is_deleted` and `deleted_at` are column
    # names, and a check that could not tell them from a `DELETE FROM` would have to
    # be relaxed the first time it fired.
    source = Path(landing.__file__).read_text(encoding="utf-8").upper()
    relation = f"{SCHEMA_QUALIFIER}.raw_records".upper()
    for forbidden in ("MERGE INTO", "UPDATE", "DELETE FROM"):
        assert f"{forbidden} {relation}" not in source, f"{forbidden} against raw_records (D7)"
    assert "ON CONFLICT" not in source, "an upsert is not an anti-join append (S4.1)"
    assert append.upper().count("INSERT") == 1, "S4.1: the write is a single statement"


def test_landing_content_hash_is_the_hashing_module_function() -> None:
    """AC4: one implementation under two spellings, and it is the same object.

    S4.1 names the function ``er.ingest.landing.content_hash`` while S3 puts the
    module at ``ingest/hashing.py``; ER-029 owns the algorithm and its committed
    golden vector, so this name must be a binding and never a wrapper.
    """
    assert landing.content_hash is hashing.content_hash
    assert "content_hash" in landing.__all__


def test_full_refresh_keys_is_refused_rather_than_silently_ignored(tmp_path: Path) -> None:
    """S4.1.1's deletion arm is a later ticket, and an unwritten arm is exit 1.

    Never `10`: "nothing to do" does not abort an `er run-all` chain, so a
    ``--full-refresh-keys`` delivery that quietly tombstoned nothing would let the
    chain continue against a corpus the operator believes was pruned.
    """
    result = invoke("ingest", "--source", "crm", "--path", str(tmp_path), "--full-refresh-keys")

    assert result.exit_code == int(ExitCode.STAGE_FAILURE)
    assert result.exit_code != int(ExitCode.NOTHING_TO_DO)
    assert "--full-refresh-keys is not implemented" in result.stderr


def test_manifest_renders_the_same_five_numbers_under_three_spellings() -> None:
    """S4.1: five counts, three renderings, and no sixth counter anywhere.

    The `ingest_batches` columns, the S4.1.1 counter names and the S4.0 manifest
    labels are checked against each other rather than against a transcription, which
    is what would catch a sixth number being introduced in one of the three.
    """
    manifest = landing.IngestManifest(
        ingest_batch_id="01J",
        run_id="01K",
        source_system="crm",
        path="/app/drop",
        new_count=3,
        changed_count=2,
        unchanged_count=1,
        tombstone_count=0,
        resurrected_count=0,
        full_refresh_keys=False,
        created_at=landing._now(),
        files=1,
        rows_in=6,
        rows_out=5,
    )

    assert set(manifest.row()) == set(REGISTRY["ingest_batches"].column_names)
    assert tuple(manifest.manifest()) == (
        "source",
        "files",
        "new",
        "changed",
        "unchanged",
        "tombstoned",
        "resurrected",
        "ingest_batch_id",
    )
    # S4.1's "same five numbers under three spellings": the short manifest label, the
    # counter name and the `ingest_batches` column, paired explicitly because the
    # short labels are not a suffix rule -- `tombstoned` is `tombstone_count`.
    spellings = (
        ("new", "new_count"),
        ("changed", "changed_count"),
        ("unchanged", "unchanged_count"),
        ("tombstoned", "tombstone_count"),
        ("resurrected", "resurrected_count"),
    )
    for short, long in spellings:
        assert manifest.manifest()[short] == manifest.counters()[long] == manifest.row()[long]

    # S4.0: `10` is exactly "new = 0 and changed = 0 and tombstoned = 0". A delivery
    # that only changed known records is a `0`, not a no-op.
    assert manifest.exit_code == int(ExitCode.SUCCESS)
    assert replace(manifest, new_count=0).exit_code == int(ExitCode.SUCCESS)
    idle = replace(manifest, new_count=0, changed_count=0, unchanged_count=6)
    assert idle.exit_code == int(ExitCode.NOTHING_TO_DO)
