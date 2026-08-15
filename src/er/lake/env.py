"""The one reader of the S6.1 ``ER_*`` environment set (DesignDoc.md S4.0b, S6.1).

S4.0b requires a single helper that reads a variable from ``os.environ`` and raises
``ERR_ENV_MISSING: <name>`` (exit ``2``) when it is absent or empty "rather than
emitting an empty literal". That failure mode is the reason the helper exists: an
empty ``ER_S3_SECRET_ACCESS_KEY`` renders as a perfectly valid empty SQL literal and
attaches a lake that authenticates as nobody, failing far from its cause.

This module is the definition site of that read. ``lake/ducklake.py`` (ER-016) builds
its SQL literals *on top of* these functions rather than beside them, so there is one
env convention and one spelling of the failure.

Nothing here imports ``er.errors``: every exception below exposes an int ``code``,
which is exactly the convention :func:`er.errors.exit_code_for` honours for the
modules that must not drag the taxonomy into their import graph.
"""

from __future__ import annotations

import os
import re
from typing import ClassVar, Final

__all__ = [
    "EnvError",
    "InvalidEnvError",
    "MissingEnvError",
    "require_bool_env",
    "require_env",
    "require_int_env",
]

# An optionally-signed run of ASCII digits and nothing else. `int()` is far more
# permissive than the SQL positions these values reach: it accepts `1_0` as ten,
# accepts surrounding whitespace, and accepts non-ASCII decimal digits. It also
# rejects `2; DROP` — but only after `int()` has already been trusted with the
# string, which is the wrong order for a value about to be pasted into a `SET`.
_INTEGER = re.compile(r"[+-]?[0-9]+")

# S7.1 writes `ER_S3_USE_SSL: "false"`, and S4.0b emits it unquoted as a SQL
# boolean. Only the two SQL spellings are accepted: `1`/`0`/`yes` would each have to
# be translated on the way to DuckDB, and a second spelling is a second convention.
_BOOLEANS: Final[dict[str, bool]] = {"true": True, "false": False}


class EnvError(Exception):
    """An ``ER_*`` variable is absent, empty, or not of the required shape.

    ``code`` is ``2`` for every subclass: S4.0's exit table makes a missing or
    invalid environment a config/validation error, and S6.1 says so directly ("A
    missing required variable exits ``2``").
    """

    #: Read by :func:`er.errors.exit_code_for`, which is why this is a plain int
    #: rather than an ``ExitCode`` import.
    code: ClassVar[int] = 2

    def __init__(self, message: str, *, name: str) -> None:
        super().__init__(message)
        self.name = name


class MissingEnvError(EnvError):
    """The variable is unset or empty.

    The message is exactly ``ERR_ENV_MISSING: <name>`` — S4.0b names that literal,
    and operators and tests match on it, so it is built here and nowhere else.
    """

    def __init__(self, name: str) -> None:
        super().__init__(f"ERR_ENV_MISSING: {name}", name=name)


class InvalidEnvError(EnvError):
    """The variable is set but is not the integer or boolean the position requires.

    S4.0b requires integer and boolean positions to be "validated and emitted
    unquoted" but names no literal for the failure, so this message is a local
    convention: ``ERR_ENV_INVALID: <name>`` in the same shape as its sibling.
    """

    def __init__(self, name: str, value: str, expected: str) -> None:
        super().__init__(f"ERR_ENV_INVALID: {name} is not {expected} (got {value!r})", name=name)
        self.value = value


def require_env(name: str) -> str:
    """Return ``$name``, raising :class:`MissingEnvError` if it is absent or empty.

    A whitespace-only value counts as empty: it renders as a non-empty SQL literal
    while carrying no value, which is the failure this function exists to prevent.
    The value itself is returned unstripped — trailing space in a secret is the
    caller's business, not this function's.
    """
    value = os.environ.get(name)
    if value is None or not value.strip():
        raise MissingEnvError(name)
    return value


def require_int_env(name: str) -> int:
    """Return ``$name`` as an int, rejecting anything that is not a plain integer."""
    value = require_env(name)
    if not _INTEGER.fullmatch(value.strip()):
        raise InvalidEnvError(name, value, "an integer")
    return int(value.strip())


def require_bool_env(name: str) -> bool:
    """Return ``$name`` as a bool, accepting only the SQL spellings ``true``/``false``."""
    value = require_env(name)
    normalized = value.strip().lower()
    if normalized not in _BOOLEANS:
        raise InvalidEnvError(name, value, "'true' or 'false'")
    return _BOOLEANS[normalized]
