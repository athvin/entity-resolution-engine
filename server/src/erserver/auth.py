"""AuthN/AuthZ: org-scoped API keys, roles, the operator token, and the audit log.

docs/backend-design.md §9, walking-skeleton depth: machines hold org-scoped API
keys (hashed at rest, shown once at issue time) with a role of ``admin``,
``steward`` or ``viewer``; the platform operator authenticates with the static
``ERSERVER_OPERATOR_TOKEN`` and manages orgs, keys and anything a tenant admin
can. OIDC for humans is a later phase — keys are the machine seam connectors
use either way.

A key reads ``erk_<key_id>_<secret>``; only ``sha256(secret)`` is stored, so a
leaked control-plane database does not leak credentials.
"""

from __future__ import annotations

import hashlib
import hmac
import secrets
from dataclasses import dataclass
from typing import Any

import psycopg
from psycopg.types.json import Jsonb
from ulid import ULID

__all__ = [
    "AuthError",
    "Principal",
    "ROLE_RANK",
    "audit",
    "authenticate",
    "issue_key",
    "revoke_key",
]

ROLE_RANK: dict[str, int] = {"viewer": 0, "steward": 1, "admin": 2, "operator": 3}


class AuthError(Exception):
    """Authentication or authorization failed; ``status`` is the HTTP answer."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status


@dataclass(frozen=True)
class Principal:
    """Who is calling: the audit actor, the org scope, and the role."""

    actor: str
    org: str | None  # None only for the operator
    role: str

    def require(self, org: str, minimum: str) -> None:
        """Refuse unless this principal holds ``minimum`` role within ``org``."""
        if self.role == "operator":
            return
        if self.org != org:
            raise AuthError(404, f"no org {org!r}")  # scope errors do not enumerate orgs
        if ROLE_RANK[self.role] < ROLE_RANK[minimum]:
            raise AuthError(403, f"requires {minimum} role")


def _hash(secret: str) -> str:
    return hashlib.sha256(secret.encode()).hexdigest()


def issue_key(connection: psycopg.Connection, org: str, role: str) -> tuple[str, str]:
    """Create a key for ``org`` and return ``(key_id, full_key)`` — shown once."""
    if role not in ("admin", "steward", "viewer"):
        raise AuthError(422, f"unknown role {role!r}")
    key_id = str(ULID()).lower()
    secret = secrets.token_urlsafe(24)
    with connection.cursor() as cursor:
        cursor.execute(
            "INSERT INTO api_keys (key_id, org, role, key_hash) VALUES (%s, %s, %s, %s)",
            (key_id, org, role, _hash(secret)),
        )
    connection.commit()
    return key_id, f"erk_{key_id}_{secret}"


def revoke_key(connection: psycopg.Connection, org: str, key_id: str) -> bool:
    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE api_keys SET revoked_at = now() "
            "WHERE key_id = %s AND org = %s AND revoked_at IS NULL",
            (key_id, org),
        )
        revoked = cursor.rowcount == 1
    connection.commit()
    return revoked


def authenticate(
    connection: psycopg.Connection,
    authorization: str | None,
    *,
    operator_token: str | None,
) -> Principal:
    """Resolve the ``Authorization: Bearer …`` header to a :class:`Principal`.

    The operator token is compared constant-time; API keys resolve through their
    stored hash. Every failure is 401 with the same message — authentication
    errors do not explain themselves.
    """
    if authorization is None or not authorization.startswith("Bearer "):
        raise AuthError(401, "missing bearer credential")
    credential = authorization.removeprefix("Bearer ").strip()
    if operator_token and hmac.compare_digest(credential, operator_token):
        return Principal(actor="operator", org=None, role="operator")
    parts = credential.split("_", 2)
    if len(parts) != 3 or parts[0] != "erk":
        raise AuthError(401, "invalid credential")
    key_id, secret = parts[1], parts[2]
    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT org, role, key_hash FROM api_keys WHERE key_id = %s AND revoked_at IS NULL",
            (key_id,),
        )
        row = cursor.fetchone()
    if row is None or not hmac.compare_digest(row[2], _hash(secret)):
        raise AuthError(401, "invalid credential")
    return Principal(actor=f"key:{key_id}", org=row[0], role=row[1])


def audit(
    connection: psycopg.Connection,
    actor: str,
    org: str | None,
    action: str,
    detail: dict[str, Any] | None = None,
) -> None:
    """One row per authenticated mutation; pairs with the lake's entity_events."""
    with connection.cursor() as cursor:
        cursor.execute(
            "INSERT INTO audit_log (actor, org, action, detail) VALUES (%s, %s, %s, %s)",
            (actor, org, action, Jsonb(detail or {})),
        )
    connection.commit()
