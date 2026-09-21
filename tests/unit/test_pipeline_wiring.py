from pathlib import Path

from er import cli


def test_run_all_constructs_real_stages_and_forwards_options() -> None:
    stages = cli.run_all_chain(
        "incremental", False, source="crm", path=Path("/tmp/drop"), reason="correction_pass"
    )
    assert [type(stage) for stage in stages] == [
        cli._IngestStage,
        cli._StandardizeStage,
        cli._MatchStage,
        cli._ReconcileStage,
        cli._AssembleStage,
    ]
    assert stages[0].source == "crm"
    assert stages[0].path == Path("/tmp/drop")
    assert stages[1].changed_only
    assert stages[2].mode == "incremental"
    assert stages[3].reason == "correction_pass"
    assert stages[4].touched_only
