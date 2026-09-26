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
import re
import secrets
from dataclasses import dataclass, replace
from typing import Any

import psycopg
from psycopg.rows import dict_row
from psycopg.types.json import Jsonb
from ulid import ULID

__all__ = [
    "AuthError",
    "Principal",
    "ROLE_RANK",
    "audit",
    "audit_rows",
    "authenticate",
    "issue_key",
    "revoke_key",
    "with_acting_user",
]

# What an X-Acting-User value may look like: an email or opaque user id from the
# web tier. Deliberately narrow — this string lands verbatim in audit rows and
# in the lake's created_by/resolved_by columns.
ACTING_USER = re.compile(r"^[\w.@+-]{1,120}$")

ROLE_RANK: dict[str, int] = {"viewer": 0, "steward": 1, "admin": 2, "operator": 3}


class AuthError(Exception):
    """Authentication or authorization failed; ``status`` is the HTTP answer."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status


@dataclass(frozen=True)
class Principal:
    """Who is calling: the audit actor, the org scope, and the role.

    ``acting_user`` is the person a trusted intermediary (the web tier) acts
    for; credentials themselves are machines, so person-level attribution can
    only arrive as an assertion alongside the credential (design §7.0).
    """

    actor: str
    org: str | None  # None only for the operator
    role: str
    acting_user: str | None = None

    @property
    def effective_actor(self) -> str:
        """The attribution string mutations record: person first, mechanism second."""
        if self.acting_user is None:
            return self.actor
        return f"user:{self.acting_user} via {self.actor}"

    def require(self, org: str, minimum: str) -> None:
        """Refuse unless this principal holds ``minimum`` role within ``org``."""
        if self.role == "operator":
            return
        if self.org != org:
            raise AuthError(404, f"no org {org!r}")  # scope errors do not enumerate orgs
        if ROLE_RANK[self.role] < ROLE_RANK[minimum]:
            raise AuthError(403, f"requires {minimum} role")


def with_acting_user(principal: Principal, value: str | None) -> Principal:
    """Bind a validated ``X-Acting-User`` assertion onto the principal.

    Any authenticated caller may assert one: API keys are bearer secrets held by
    trusted services (today, only the web tier), and the mechanism half of
    :attr:`Principal.effective_actor` always preserves *which* credential made
    the assertion, so a spoofed header incriminates its own key.
    """
    if value is None:
        return principal
    if not ACTING_USER.match(value):
        raise AuthError(422, "X-Acting-User must match [\\w.@+-]{1,120}")
    return replace(principal, acting_user=value)


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


def audit_rows(
    connection: psycopg.Connection,
    *,
    org: str | None = None,
    action: str | None = None,
    before_id: int | None = None,
    limit: int = 100,
) -> list[dict[str, Any]]:
    """Read the audit ledger, newest first; ``before_id`` is the keyset cursor."""
    clauses = ["TRUE"]
    params: list[Any] = []
    if org is not None:
        clauses.append("org = %s")
        params.append(org)
    if action is not None:
        clauses.append("action = %s")
        params.append(action)
    if before_id is not None:
        clauses.append("id < %s")
        params.append(before_id)
    params.append(limit)
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            "SELECT id, at, actor, org, action, detail FROM audit_log "
            f"WHERE {' AND '.join(clauses)} ORDER BY id DESC LIMIT %s",
            params,
        )
        return list(cursor.fetchall())
