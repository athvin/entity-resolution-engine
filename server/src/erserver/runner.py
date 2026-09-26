"""The runner: one process, one pipeline run, one terminal result line.

Launched by the dispatcher with the tenant's ``ER_*`` environment already
injected (docs/backend-design.md §4) and a single JSON argument describing the
job. It executes the run in-process through :mod:`er.service` — the same lock
window, drift guard and chain the CLI runs — prints one machine-readable JSON
line to stdout, and exits with the run's S4.0 status. stderr stays what it is
everywhere in the engine: telemetry, one JSON line per stage.
"""

from __future__ import annotations

import json
import sys
from dataclasses import asdict
from pathlib import Path
from typing import Any

__all__ = ["execute", "main"]


def execute(payload: dict[str, Any]) -> dict[str, Any]:
    """Run the job the payload describes and return the terminal result record."""
    # Imported here, not at module top: the engine attaches nothing at import
    # time, but a runner that fails argument parsing should fail before paying
    # for the import graph.
    from er import service

    kind = payload["kind"]
    params: dict[str, Any] = payload.get("params", {})
    config_path = Path(payload["config_path"])
    run_id: str | None = payload.get("run_id")
    resume: str | None = params.get("resume_run_id")

    if kind in ("run_all_full", "run_all_incremental"):
        outcome = service.run_pipeline(
            mode="full" if kind == "run_all_full" else "incremental",
            config_path=config_path,
            run_id=run_id,
            source=params.get("source"),
            path=params.get("path"),
            skip_ingest=bool(params.get("skip_ingest", False)),
            allow_escalate=bool(params.get("allow_escalate", False)),
            resume=resume,
            reason=params.get("reason"),
        )
    elif kind == "correct":
        outcome = service.run_correction(
            config_path=config_path,
            run_id=None if resume is not None else run_id,
            resume=resume,
        )
    elif kind == "train":
        outcome = service.run_training(
            config_path=config_path,
            run_id=run_id,
            if_changed=bool(params.get("if_changed", False)),
        )
    else:
        raise ValueError(f"unknown job kind: {kind!r}")

    record = asdict(outcome)
    record["job_id"] = payload.get("job_id")
    return record


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        sys.stderr.write("usage: python -m erserver.runner '<job payload json>'\n")
        return 2
    payload = json.loads(argv[1])
    record = execute(payload)
    sys.stdout.write(json.dumps(record, separators=(",", ":")) + "\n")
    return int(record["exit_code"])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
