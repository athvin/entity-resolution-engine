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
import os
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
    elif kind == "lake_maintain":
        maintain_kwargs: dict[str, Any] = {}
        if "retain_days" in params:
            maintain_kwargs["retain_days"] = int(params["retain_days"])
        outcome = service.run_maintenance(config_path=config_path, run_id=run_id, **maintain_kwargs)
    elif kind == "provision":
        from erserver import provision

        outcome = provision.execute(params, run_id=run_id or str(payload.get("job_id")))
    else:
        raise ValueError(f"unknown job kind: {kind!r}")

    record = asdict(outcome)
    record["job_id"] = payload.get("job_id")
    return record


def persist_result(record: dict[str, Any]) -> None:
    """File the terminal record on the job row (``progress.result``).

    The stdout result line rides the pod log, and the pod log dies with its
    node — WhenEmpty reclaims runner nodes 30 seconds after the Job finishes,
    sometimes before the dispatcher's one tail fetch. The job row does not die,
    so this copy is what keeps the exit-code taxonomy's ``error_class`` whole.
    Best-effort by design: a run must never fail because its receipt could not
    be filed, and outside the server (no ``ERSERVER_DSN``) there is no row.
    """
    dsn = os.environ.get("ERSERVER_DSN", "")
    job_id = record.get("job_id")
    if not dsn or not job_id:
        return
    try:
        import psycopg
        from psycopg.types.json import Jsonb

        with psycopg.connect(dsn, connect_timeout=10) as connection:
            with connection.cursor() as cursor:
                cursor.execute(
                    "UPDATE jobs SET progress = progress || %s, updated_at = now() "
                    "WHERE job_id = %s",
                    (Jsonb({"result": record}), str(job_id)),
                )
    except Exception as exc:  # noqa: BLE001 — the receipt is best-effort
        sys.stderr.write(f"runner: could not persist the terminal result: {exc}\n")


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        sys.stderr.write("usage: python -m erserver.runner '<job payload json>'\n")
        return 2
    payload = json.loads(argv[1])
    record = execute(payload)
    persist_result(record)
    sys.stdout.write(json.dumps(record, separators=(",", ":")) + "\n")
    return int(record["exit_code"])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
