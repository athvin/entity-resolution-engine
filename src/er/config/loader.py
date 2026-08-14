"""Reading and validating the S6 configuration document (DesignDoc.md S6.1, S4.0).

S4.0 validates the document at process start, before any lake connection is opened,
and exits ``2`` on failure. Nothing here opens a connection or imports an engine, so
that ordering is a property of the import graph rather than a convention.

Every failure raised here carries ``code = 2`` as an attribute, so the CLI maps it
to the S4.0 exit status without importing this module's types, and carries the
offending JSON pointer, which S4.0 requires the process to print.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, ClassVar

import yaml
from pydantic import ValidationError
from pydantic_core import ErrorDetails

from er.config.schema import Config, normalize

__all__ = ["CONFIG_ENV_VAR", "ConfigError", "ConfigValidationError", "load_config"]

#: The one environment variable this package reads. Every other ``ER_*`` name in
#: S6.1's environment-variables paragraph belongs to the lake, not to the document.
CONFIG_ENV_VAR = "ER_CONFIG"

# Pydantic renders a validator's ``ValueError`` as "Value error, <our message>".
_VALUE_ERROR_PREFIX = "Value error, "

# Every S6.1 failure message key is a dotted lowercase token, and the schema's
# validators put theirs first in the message. Matching the shape rather than a
# fixed list means the ticket that adds V1-V8 does not have to edit this module.
_MESSAGE_KEY_RE = re.compile(r"(?P<key>[a-z0-9_]+(?:\.[a-z0-9_]+)+):")


@dataclass(frozen=True)
class ConfigError:
    """One rejected location in the document."""

    #: The Pydantic error path, e.g. ``("training", "u_seed")``.
    loc: tuple[str | int, ...]
    #: The S6.1 failure message key, e.g. ``"training.u_seed.required"``.
    key: str
    message: str

    @property
    def pointer(self) -> str:
        """``loc`` as the JSON pointer S4.0 prints."""
        return "/" + "/".join(str(part) for part in self.loc)


class ConfigValidationError(Exception):
    """A document that could not be read, parsed, or validated against S6.

    ``code`` is the S4.0 exit status. It is a class attribute so a caller can read
    it off the instance without importing this type.
    """

    code: ClassVar[int] = 2

    def __init__(self, message: str, errors: tuple[ConfigError, ...] = ()) -> None:
        super().__init__(message)
        self.errors = errors

    @property
    def pointer(self) -> str:
        """The pointer of the first rejected location; ``/`` for a whole-document failure."""
        return self.errors[0].pointer if self.errors else "/"


def resolve_path(path: str | os.PathLike[str] | None = None) -> Path:
    """The document to load: the explicit ``--config`` path, else ``$ER_CONFIG`` (S4.0)."""
    if path is not None:
        return Path(path)
    from_env = os.environ.get(CONFIG_ENV_VAR)
    if not from_env:
        raise ConfigValidationError(f"no config path given and {CONFIG_ENV_VAR} is unset")
    return Path(from_env)


def read_document(path: Path) -> dict[str, Any]:
    """Parse the YAML document into a mapping, or raise with the S4.0 exit code."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ConfigValidationError(f"cannot read config at {path}: {exc}") from exc
    try:
        loaded = yaml.safe_load(text)
    except yaml.YAMLError as exc:
        raise ConfigValidationError(f"{path} is not valid YAML: {exc}") from exc
    if not isinstance(loaded, dict):
        raise ConfigValidationError(
            f"{path} must contain a YAML mapping, found {type(loaded).__name__}"
        )
    return {str(key): value for key, value in loaded.items()}


def _message_key(error: ErrorDetails) -> str:
    """The S6.1 failure message key a Pydantic error belongs to."""
    path = ".".join(str(part) for part in error["loc"])
    # A required field with no default is V10's whole story for `training.u_seed`,
    # and the key S6.1 gives it is exactly this construction.
    if error["type"] == "missing":
        return f"{path}.required" if path else "required"
    message = error["msg"].removeprefix(_VALUE_ERROR_PREFIX)
    match = _MESSAGE_KEY_RE.match(message)
    if match is not None:
        return match.group("key")
    # A type or constraint error Pydantic raised on its own: no S6.1 row names it,
    # so the key is derived from where it happened rather than invented.
    return f"{path}.{error['type']}" if path else error["type"]


def load_config(path: str | os.PathLike[str] | None = None) -> Config:
    """Read, validate and normalize the S6 document.

    Args:
        path: the document to load; ``$ER_CONFIG`` when omitted.

    Returns:
        The validated document with S6.1 normalization applied, which is the form
        ``config_hash`` is computed over (S5.2).

    Raises:
        ConfigValidationError: the document is unreadable, is not YAML, or fails
            S6 validation. ``code`` is ``2`` and ``pointer`` is the offending JSON
            pointer, as S4.0 requires.
    """
    document_path = resolve_path(path)
    document = read_document(document_path)
    try:
        config = Config.model_validate(document)
    except ValidationError as exc:
        errors = tuple(
            ConfigError(
                loc=tuple(error["loc"]),
                key=_message_key(error),
                message=error["msg"].removeprefix(_VALUE_ERROR_PREFIX),
            )
            for error in exc.errors()
        )
        summary = "; ".join(f"{error.pointer}: {error.key}" for error in errors)
        raise ConfigValidationError(
            f"{document_path} failed S6 validation: {summary}", errors
        ) from exc
    return normalize(config)
