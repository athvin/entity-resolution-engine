"""Control-plane configuration, read from ``ERSERVER_*`` environment variables.

Deliberately separate from the engine's ``ER_*`` set: the control-plane database
is not the DuckLake catalog, and a runner's per-tenant lake environment is
injected per process by the dispatcher (docs/backend-design.md §7), never read
from the server's own environment.
"""

from __future__ import annotations

import os
from dataclasses import dataclass

__all__ = ["MissingSettingError", "ServerSettings"]


class MissingSettingError(RuntimeError):
    """A required ``ERSERVER_*`` variable is absent or empty."""

    def __init__(self, name: str) -> None:
        super().__init__(f"ERSERVER_SETTING_MISSING: {name}")
        self.name = name


def _require(name: str) -> str:
    value = os.environ.get(name)
    if value is None or not value.strip():
        raise MissingSettingError(name)
    return value


@dataclass(frozen=True)
class ServerSettings:
    """Everything the API and dispatcher need to start."""

    #: Postgres DSN of the control-plane database (orgs, jobs, schedules).
    dsn: str
    #: Dispatcher poll interval when the queue was empty, in seconds.
    poll_seconds: float = 2.0
    #: The platform operator's bearer token; ``None`` disables operator auth
    #: (every operator endpoint then refuses).
    operator_token: str | None = None
    #: Concurrent runner slots. Per-org serialization is constraint-backed in
    #: the queue, so concurrency here only ever parallelizes ACROSS orgs.
    concurrency: int = 2

    @classmethod
    def from_env(cls) -> ServerSettings:
        poll = os.environ.get("ERSERVER_POLL_SECONDS")
        workers = os.environ.get("ERSERVER_CONCURRENCY")
        return cls(
            dsn=_require("ERSERVER_DSN"),
            poll_seconds=float(poll) if poll else 2.0,
            operator_token=os.environ.get("ERSERVER_OPERATOR_TOKEN") or None,
            concurrency=max(1, int(workers)) if workers else 2,
        )
