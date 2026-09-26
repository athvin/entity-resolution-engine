"""Config versions: validate, classify the change's cost tier, publish.

docs/backend-design.md §6: config editing is a publish workflow, not a file
save, because any edit changes ``config_hash`` and the engine's drift guard
refuses the next incremental run. A draft is validated with the engine's own
loader (same pydantic models, same JSON-pointer errors); publishing classifies
the diff into a cost tier, writes the file the runner loads, syncs the
correction-pass schedule the document declares, and enqueues the rebuild the
tier requires — turning the drift guard into the audit spine.

Tier vocabulary (§6):

- **A** — re-band / re-assemble scale: ``thresholds``, ``survivorship``.
- **B** — match rebuild, no retrain: ``blocking``.
- **C** — retrain + full rebuild: ``comparisons``, ``standardization``,
  ``sources``.
- Operator-only blocks (``tenant``, ``storage``, ``versions``, ``training``,
  ``clustering``, ``coherence``, ``generator``) refuse tenant publish outright.
- ``correction_pass`` alone changes no data — it only re-syncs the schedule.

The engine rebuilds everything on any config change (the guard escalates), so
today every tier enqueues a full run — tier C additionally trains first. The
tier is still computed and recorded: it is the honest cost estimate the UI
shows, and the seam a cheaper tier-A path plugs into later.
"""

from __future__ import annotations

import os
import tempfile
from pathlib import Path
from typing import Any

import psycopg
from psycopg.rows import dict_row

from er.config.hashing import config_hash
from er.config.loader import load_config
from er.config.schema import Config

__all__ = [
    "OPERATOR_ONLY_BLOCKS",
    "PublishRefused",
    "active_config",
    "classify_tier",
    "create_version",
    "get_version",
    "list_versions",
    "publish",
]

TIER_A_BLOCKS = frozenset({"thresholds", "survivorship"})
TIER_B_BLOCKS = frozenset({"blocking"})
TIER_C_BLOCKS = frozenset({"comparisons", "standardization", "sources"})
OPERATOR_ONLY_BLOCKS = frozenset(
    {"tenant", "storage", "versions", "training", "clustering", "coherence", "generator"}
)


class PublishRefused(Exception):
    """The publish is not allowed as requested; ``status`` is the HTTP answer."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status


def validate_yaml(yaml_text: str) -> tuple[Config, str]:
    """Validate through the engine's loader; returns ``(config, config_hash)``.

    Raises :class:`er.config.loader.ConfigValidationError` with the offending
    JSON pointer, exactly as ``er --config`` would.
    """
    with tempfile.NamedTemporaryFile("w", suffix=".yaml", delete=False) as handle:
        handle.write(yaml_text)
        temp_path = Path(handle.name)
    try:
        document = load_config(temp_path)
    finally:
        temp_path.unlink(missing_ok=True)
    return document, config_hash(document)


def classify_tier(old: Config | None, new: Config) -> tuple[str | None, list[str], list[str]]:
    """``(tier, changed_blocks, operator_only_changed)`` for old → new.

    ``tier`` is ``None`` when nothing rebuild-relevant changed (first publish
    of an org counts as tier C — everything must be built).
    """
    if old is None:
        return "C", ["initial"], []
    old_dump = old.model_dump(mode="json")
    new_dump = new.model_dump(mode="json")
    changed = [block for block in old_dump if old_dump[block] != new_dump.get(block)]
    operator_only = [block for block in changed if block in OPERATOR_ONLY_BLOCKS]
    if any(block in TIER_C_BLOCKS for block in changed) or operator_only:
        tier: str | None = "C"
    elif any(block in TIER_B_BLOCKS for block in changed):
        tier = "B"
    elif any(block in TIER_A_BLOCKS for block in changed):
        tier = "A"
    else:
        tier = None
    return tier, changed, operator_only


def create_version(
    connection: psycopg.Connection, org: str, yaml_text: str, created_by: str
) -> dict[str, Any]:
    """Store a validated draft; version numbers are per-org and monotonic."""
    _, digest = validate_yaml(yaml_text)
    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT coalesce(max(version), 0) + 1 FROM config_versions WHERE org = %s", (org,)
        )
        row = cursor.fetchone()
        assert row is not None
        version = int(row[0])
        cursor.execute(
            "INSERT INTO config_versions (org, version, yaml, config_hash, created_by) "
            "VALUES (%s, %s, %s, %s, %s)",
            (org, version, yaml_text, digest, created_by),
        )
    connection.commit()
    return {"org": org, "version": version, "config_hash": digest, "state": "draft"}


def list_versions(connection: psycopg.Connection, org: str) -> list[dict[str, Any]]:
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            "SELECT version, config_hash, state, tier, created_by, created_at, published_at "
            "FROM config_versions WHERE org = %s ORDER BY version DESC",
            (org,),
        )
        return list(cursor.fetchall())


def get_version(connection: psycopg.Connection, org: str, version: int) -> dict[str, Any] | None:
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            "SELECT version, yaml, config_hash, state, tier, created_by, created_at, "
            "published_at FROM config_versions WHERE org = %s AND version = %s",
            (org, version),
        )
        return cursor.fetchone()


def active_config(connection: psycopg.Connection, org: str) -> dict[str, Any] | None:
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute("SELECT active_config_version FROM orgs WHERE name = %s", (org,))
        row = cursor.fetchone()
    if row is None or row["active_config_version"] is None:
        return None
    return get_version(connection, org, int(row["active_config_version"]))


def publish(
    connection: psycopg.Connection,
    org: str,
    version: int,
    *,
    actor: str,
    is_operator: bool,
) -> dict[str, Any]:
    """Make ``version`` the org's live document and enqueue what it costs.

    Writes the YAML to the org's ``config_path`` (the file every runner
    invocation loads), stamps the version published, syncs the correction
    schedule, and enqueues: tier C → a train job then a full run; tier A/B →
    a full run (the drift guard would escalate an incremental anyway); tier
    ``None`` → nothing.
    """
    from erserver import queue, schedules

    draft = get_version(connection, org, version)
    if draft is None:
        raise PublishRefused(404, f"no config version {version} for org {org!r}")
    new_config, _ = validate_yaml(draft["yaml"])

    current = active_config(connection, org)
    old_config = None if current is None else validate_yaml(current["yaml"])[0]
    tier, changed, operator_only = classify_tier(old_config, new_config)
    if operator_only and not is_operator:
        raise PublishRefused(
            403, f"blocks {sorted(operator_only)} are operator-only; publish refused"
        )

    org_target = queue.org_row(connection, org)
    if org_target is None:
        raise PublishRefused(404, f"unknown org: {org!r}")
    config_path = Path(org_target[0])
    config_path.parent.mkdir(parents=True, exist_ok=True)
    # Atomic swap: a runner starting mid-publish reads the old document or the
    # new one, never a torn write.
    descriptor, staged = tempfile.mkstemp(dir=config_path.parent, suffix=".yaml.tmp")
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        handle.write(draft["yaml"])
    os.replace(staged, config_path)

    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE config_versions SET state = 'published', tier = %s, published_at = now() "
            "WHERE org = %s AND version = %s",
            (tier, org, version),
        )
        cursor.execute("UPDATE orgs SET active_config_version = %s WHERE name = %s", (version, org))
    connection.commit()

    schedules.sync_correction_schedule(connection, org, new_config.correction_pass.cadence)

    enqueued: list[str] = []
    if tier == "C":
        job = queue.enqueue(connection, org, "train", idempotency_key=f"publish:{version}:train")
        enqueued.append(job.job_id)
    if tier is not None:
        job = queue.enqueue(
            connection,
            org,
            "run_all_full",
            params={"skip_ingest": True},
            idempotency_key=f"publish:{version}:rebuild",
        )
        enqueued.append(job.job_id)

    return {
        "org": org,
        "version": version,
        "tier": tier,
        "changed_blocks": changed,
        "jobs_enqueued": enqueued,
        "published_by": actor,
    }
