import json
import subprocess
import sys


def test_nonmatching_cli_and_blocking_payload_do_not_import_splink() -> None:
    completed = subprocess.run(
        [
            sys.executable,
            "-c",
            "import sys,json; import er.cli; "
            "from er.config.loader import load_config; "
            "from er.std.blocking import blocking_payload; "
            "assert blocking_payload(load_config('configs/test.yaml')); "
            "print(json.dumps({'splink': 'splink' in sys.modules, "
            "'pandas': 'pandas' in sys.modules}))",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    assert json.loads(completed.stdout) == {"splink": False, "pandas": False}
