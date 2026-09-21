import json
from pathlib import Path

import pytest

from er import cli, entrypoint


@pytest.mark.parametrize("code", [0, 10, 1, 3])
def test_entrypoint_records_real_exit_status(
    code: int, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("ER_PROFILE_DIR", str(tmp_path))

    def exit_main() -> None:
        raise SystemExit(code)

    monkeypatch.setattr(cli, "main", exit_main)
    with pytest.raises(SystemExit) as raised:
        entrypoint.main()
    assert raised.value.code == code
    events = [
        json.loads(line)
        for path in tmp_path.glob("events-*.jsonl")
        for line in path.read_text().splitlines()
    ]
    completed = next(
        event
        for event in events
        if event["event"] == "span_end" and event["name"] == "command.execution"
    )
    assert completed["status"] == ("succeeded" if code in (0, 10) else "failed")
