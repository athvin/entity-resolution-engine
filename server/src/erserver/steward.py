"""Steward writes: review resolution and assertions, staged when the lock is busy.

docs/backend-design.md §16 problem 2: a pipeline run holds the tenant's writer
lock for minutes-to-hours, and a steward's merge click must not block on it.
The write is tried inline first — the common case finds the lock free and lands
sub-second — and a lock conflict stages the action in the control plane, where
the dispatcher drains it as soon as the org goes quiet. Semantically honest:
assertions only take effect at the next reconcile anyway, so "queued, applies
after the current job" is the truth, not a workaround.

The engine primitives do all the real work (`er.review.queue`,
`er.review.assertions`); this module owns only the lock window and the staging
table.
"""

from __future__ import annotations

import json
from typing import Any

import psycopg
from psycopg.rows import dict_row
from psycopg.types.json import Jsonb
from ulid import ULID

from er.errors import ErError, PreconditionFailure
from er.lake.catalog import tenant_lock
from er.lake.ducklake import connect, invocation_session
from er.lake.env import lake_environment

__all__ = [
    "apply_action",
    "drain_org",
    "pending_count",
    "stage_action",
    "try_apply_or_stage",
]

ACTION_TYPES = ("resolve_review", "add_assertion", "retract_assertion")


def _perform(action: dict[str, Any], actor: str) -> dict[str, Any]:
    """Run one steward primitive on an open lake connection; returns a summary."""
    kind = action.get("type")
    with connect() as connection:
        if kind == "resolve_review":
            from er.review.queue import resolve_review

            resolution = resolve_review(
                connection,
                action["review_id"],
                resolution=action["resolution"],
                resolved_by=actor,
            )
            written = resolution.assertion
            return {
                "applied": "resolve_review",
                "review_id": action["review_id"],
                "resolution": action["resolution"],
                "assertion_id": None if written is None else written.assertion_id,
            }
        if kind == "add_assertion":
            from er.review.assertions import add_assertion

            assertion = add_assertion(
                connection,
                a=action["a"],
                b=action["b"],
                kind=action["kind"],
                created_by=actor,
                note=action.get("note"),
            )
            return {
                "applied": "add_assertion",
                "assertion_id": assertion.assertion_id,
                "kind": action["kind"],
            }
        if kind == "retract_assertion":
            from er.review.assertions import retract_assertion

            retracted = retract_assertion(
                connection,
                action["assertion_id"],
                retracted_by=actor,
            )
            return {"applied": "retract_assertion", "assertion_id": retracted.assertion_id}
    raise ValueError(f"unknown steward action type: {kind!r}")


def apply_action(
    env: dict[str, str], tenant: str, action: dict[str, Any], actor: str
) -> dict[str, Any]:
    """Apply one action under the tenant's lock, in its lake environment.

    Raises :class:`er.errors.PreconditionFailure` when the writer lock is held —
    the caller stages the action instead. Mirrors the CLI's ``assert``/``review
    resolve`` window: lock, session, pending-correction gate with
    ``assertion_repair=True`` (steward repair stays available mid-correction).
    """
    with lake_environment(env):
        with tenant_lock(tenant, run_id=str(ULID())):
            with invocation_session():
                from er.matching.correction import assert_no_pending_correction

                with connect() as connection:
                    assert_no_pending_correction(
                        connection, resume_run_id=None, assertion_repair=True
                    )
                return _perform(action, actor)


def stage_action(
    connection: psycopg.Connection, org: str, action: dict[str, Any], created_by: str
) -> str:
    action_id = str(ULID())
    with connection.cursor() as cursor:
        cursor.execute(
            "INSERT INTO staged_steward_actions (action_id, org, action, created_by) "
            "VALUES (%s, %s, %s, %s)",
            (action_id, org, Jsonb(action), created_by),
        )
    connection.commit()
    return action_id


def pending_count(connection: psycopg.Connection, org: str) -> int:
    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT count(*) FROM staged_steward_actions WHERE org = %s AND state = 'pending'",
            (org,),
        )
        row = cursor.fetchone()
    return 0 if row is None else int(row[0])


def try_apply_or_stage(
    connection: psycopg.Connection,
    org: str,
    env: dict[str, str],
    tenant: str,
    action: dict[str, Any],
    actor: str,
) -> tuple[str, dict[str, Any]]:
    """``("applied", result)`` inline, or ``("staged", {action_id})`` on lock conflict.

    Ordering matters: if actions are already staged for this org, a new one is
    staged behind them rather than applied around them — a steward's clicks
    land in the order they were made.
    """
    if pending_count(connection, org) == 0:
        try:
            return "applied", apply_action(env, tenant, action, actor)
        except PreconditionFailure:
            pass  # the writer lock is held (or a correction is pending): stage it
    action_id = stage_action(connection, org, action, actor)
    return "staged", {"action_id": action_id}


def drain_org(connection: psycopg.Connection, org: str, env: dict[str, str], tenant: str) -> int:
    """Apply this org's pending staged actions in order; returns how many landed.

    Called by the dispatcher when the org has no active job. A lock conflict
    stops the drain (a run started); a real failure marks that action failed
    and continues, so one bad action cannot dam the queue behind it.
    """
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            "SELECT action_id, action, created_by FROM staged_steward_actions "
            "WHERE org = %s AND state = 'pending' ORDER BY action_id",
            (org,),
        )
        rows = cursor.fetchall()
    applied = 0
    for row in rows:
        action = row["action"] if isinstance(row["action"], dict) else json.loads(row["action"])
        try:
            apply_action(env, tenant, action, row["created_by"])
        except PreconditionFailure:
            break
        except (ErError, ValueError, KeyError) as exc:
            with connection.cursor() as cursor:
                cursor.execute(
                    "UPDATE staged_steward_actions SET state = 'failed', error = %s, "
                    "applied_at = now() WHERE action_id = %s",
                    (str(exc), row["action_id"]),
                )
            connection.commit()
            continue
        with connection.cursor() as cursor:
            cursor.execute(
                "UPDATE staged_steward_actions SET state = 'applied', applied_at = now() "
                "WHERE action_id = %s",
                (row["action_id"],),
            )
        connection.commit()
        applied += 1
    return applied
