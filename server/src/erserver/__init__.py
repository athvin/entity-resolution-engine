"""Control plane for the entity-resolution engine.

Three deployables in one package, per docs/backend-design.md §4:

- :mod:`erserver.api` — the FastAPI app (control-plane CRUD; never runs stages).
- :mod:`erserver.dispatcher` — claims queued jobs and launches runner processes,
  one per pipeline run, serialized per org.
- :mod:`erserver.runner` — a subprocess that executes exactly one run through
  :mod:`er.service` and reports a terminal result line.

The queue is plain Postgres (``SELECT … FOR UPDATE SKIP LOCKED``); the retry
policy is keyed to the engine's S4.0 exit-code taxonomy (:mod:`erserver.policy`).
"""

__all__ = ["__version__"]

__version__ = "0.1.0"
