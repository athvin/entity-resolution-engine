"""Secret references: the control plane stores pointers, never credentials.

docs/backend-design.md §7 puts tenant credentials in a secrets manager,
referenced by ID and injected at dispatch. This is that seam in single-VM
form: an org env value written as ``secret://NAME`` resolves — at use time,
in the server process — from ``ERSERVER_SECRET_NAME`` in the server's own
environment, which is exactly where a container platform mounts its secrets.
The database row carries only the reference, so a leaked control-plane dump
leaks no credentials; swapping this module's resolver for a cloud secrets
manager changes nothing about the stored data.

Literal values still pass through untouched, so local development can inline
what production references.
"""

from __future__ import annotations

import os
from collections.abc import Mapping

__all__ = ["SECRET_SCHEME", "UnresolvedSecretError", "resolve_env", "resolve_value"]

SECRET_SCHEME = "secret://"
_ENV_PREFIX = "ERSERVER_SECRET_"


class UnresolvedSecretError(RuntimeError):
    """A ``secret://`` reference names a secret the server does not hold.

    The message carries the reference, never a value.
    """

    def __init__(self, name: str, variable: str) -> None:
        super().__init__(
            f"unresolved secret reference {SECRET_SCHEME}{name}: "
            f"the server environment does not set {variable}"
        )
        self.name = name


def resolve_value(value: str) -> str:
    """Resolve one value: ``secret://NAME`` from the server env, else verbatim."""
    if not value.startswith(SECRET_SCHEME):
        return value
    name = value.removeprefix(SECRET_SCHEME)
    variable = f"{_ENV_PREFIX}{name}"
    resolved = os.environ.get(variable)
    if resolved is None or not resolved.strip():
        raise UnresolvedSecretError(name, variable)
    return resolved


def resolve_env(env: Mapping[str, str]) -> dict[str, str]:
    """Resolve every value of an org's stored environment overlay."""
    return {key: resolve_value(value) for key, value in env.items()}
