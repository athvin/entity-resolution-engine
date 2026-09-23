"""Time CLI import and execution without loading stage engines for telemetry."""

from er.obs.profiling import span
from er.obs.python_profile import python_profile


def main() -> None:
    with python_profile("cli"):
        _main()


def _main() -> None:
    with span("command.import"):
        from er.cli import main as cli_main
    exit_request = None
    with span("command.execution") as metrics:
        try:
            cli_main()
        except SystemExit as exc:
            metrics["exit_code"] = exc.code if isinstance(exc.code, int) else int(bool(exc.code))
            exit_request = exc
    if exit_request is not None:
        raise exit_request
