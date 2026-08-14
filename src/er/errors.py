"""The S4.7 failure taxonomy and the S4.0 exit-code contract (DesignDoc.md S4.0, S4.7).

Two tables are normative and live only here:

* :data:`ERROR_CLASS_TO_EXIT` — every value of ``run_stages.error_class`` maps to
  exactly one S4.0 exit status. No stage may pick an exit code directly; it raises
  a classified exception and the code follows from the class, so the class written
  to the lake and the code the process exits with can never disagree.
* :data:`RETRYABLE` — S4.7's "Retryable" column. It is advisory to an operator and
  to nothing else: S4.7's non-goals forbid automatic retry loops anywhere in the
  CLI, so nothing in this package may read it and re-run a stage.

Exit ``10`` sits deliberately outside the taxonomy. S4.0 makes it "nothing to do",
which is a *successful* no-op that never aborts an ``er run-all`` chain, so it has
no ``error_class`` row and :class:`NothingToDo` carries no class.

This module imports nothing else under ``src/er``: the modules that predate it
(``er.config.loader``, and ``er.lake.env`` when it lands) surface their status by
exposing an int ``code`` attribute instead of importing these types, and
:func:`exit_code_for` honours that attribute.
"""

from __future__ import annotations

from collections.abc import Mapping
from enum import IntEnum, StrEnum
from types import MappingProxyType
from typing import ClassVar

__all__ = [
    "ERROR_CLASS_TO_EXIT",
    "RETRYABLE",
    "ConfigError",
    "ErError",
    "ErrorClass",
    "ExitCode",
    "NothingToDo",
    "PreconditionFailure",
    "StageFailure",
    "exit_code_for",
]


class ExitCode(IntEnum):
    """The S4.0 exit statuses, uniform across every ``er`` command."""

    SUCCESS = 0
    STAGE_FAILURE = 1
    CONFIG = 2
    PRECONDITION = 3
    NOTHING_TO_DO = 10


class ErrorClass(StrEnum):
    """The values ``run_stages.error_class`` may take (S4.7).

    A :class:`StrEnum` because the class is written verbatim into a ``VARCHAR``
    column and into every stage's JSON log line (S5.2); the member value IS the
    stored spelling.
    """

    TRANSIENT_IO = "transient_io"
    LOCK_CONFLICT = "lock_conflict"
    PRECONDITION = "precondition"
    CONFIG = "config"
    CONTRADICTION = "contradiction"
    NON_CONVERGENCE = "non_convergence"
    DATA = "data"


#: S4.7's "Exit" column. Total over :class:`ErrorClass` by construction — a class
#: with no exit code would leave a stage unable to terminate.
ERROR_CLASS_TO_EXIT: Mapping[ErrorClass, ExitCode] = MappingProxyType(
    {
        ErrorClass.TRANSIENT_IO: ExitCode.STAGE_FAILURE,
        ErrorClass.LOCK_CONFLICT: ExitCode.PRECONDITION,
        ErrorClass.PRECONDITION: ExitCode.PRECONDITION,
        ErrorClass.CONFIG: ExitCode.CONFIG,
        ErrorClass.CONTRADICTION: ExitCode.STAGE_FAILURE,
        ErrorClass.NON_CONVERGENCE: ExitCode.STAGE_FAILURE,
        ErrorClass.DATA: ExitCode.STAGE_FAILURE,
    }
)

#: S4.7's "Retryable" column: true for exactly the two classes whose cause is
#: outside the run — a transient I/O fault and another writer holding the lock.
RETRYABLE: Mapping[ErrorClass, bool] = MappingProxyType(
    {
        ErrorClass.TRANSIENT_IO: True,
        ErrorClass.LOCK_CONFLICT: True,
        ErrorClass.PRECONDITION: False,
        ErrorClass.CONFIG: False,
        ErrorClass.CONTRADICTION: False,
        ErrorClass.NON_CONVERGENCE: False,
        ErrorClass.DATA: False,
    }
)


class ErError(Exception):
    """Base of every exception the CLI translates into an S4.0 exit status.

    Args:
        message: what failed, in the operator's terms.
        error_class: the S4.7 row this failure is recorded under; the subclass's
            :attr:`default_error_class` when omitted.
        detail: the value for ``run_stages.error_detail``; ``message`` when
            omitted, which is what a caller wants in all but the cases that have a
            long diagnosis (the CONTRADICTION-1 closure component, say) to attach.
    """

    #: Class assigned when the raiser does not name one. ``None`` only for
    #: :class:`NothingToDo`, which S4.7 does not classify because it is not a
    #: failure.
    default_error_class: ClassVar[ErrorClass | None] = ErrorClass.DATA

    #: Exit status used when :attr:`error_class` is ``None``.
    unclassified_exit: ClassVar[ExitCode] = ExitCode.STAGE_FAILURE

    def __init__(
        self,
        message: str,
        *,
        error_class: ErrorClass | None = None,
        detail: str | None = None,
    ) -> None:
        super().__init__(message)
        self.error_class: ErrorClass | None = (
            error_class if error_class is not None else type(self).default_error_class
        )
        self.detail = detail if detail is not None else message

    @property
    def code(self) -> int:
        """The S4.0 exit status, derived from :attr:`error_class`."""
        if self.error_class is None:
            return int(type(self).unclassified_exit)
        return int(ERROR_CLASS_TO_EXIT[self.error_class])

    @property
    def retryable(self) -> bool:
        """S4.7's advisory retry verdict. Nothing in the CLI acts on it."""
        return self.error_class is not None and RETRYABLE[self.error_class]


class StageFailure(ErError):
    """An operation was attempted and failed (S4.0 exit ``1``).

    The default class is ``data``; a raiser with a more precise diagnosis passes
    ``error_class=`` — ``transient_io`` for an S3 5xx, ``contradiction`` for
    CONTRADICTION-1, ``non_convergence`` for an exceeded fixpoint bound.
    """


class PreconditionFailure(ErError):
    """A precondition of the stage does not hold (S4.0 exit ``3``).

    ``lock_conflict`` is passed explicitly for the S4.0b advisory lock; the two
    classes share an exit code but not an operator action.
    """

    default_error_class: ClassVar[ErrorClass | None] = ErrorClass.PRECONDITION


class ConfigError(ErError):
    """The configuration or the invocation is invalid (S4.0 exit ``2``).

    ``er.config.loader.ConfigValidationError`` is the S6.1 arm of the same code
    and is deliberately NOT a subclass: that module imports nothing from here so
    that config validation cannot drag the taxonomy into the import graph before
    the document has been read.
    """

    default_error_class: ClassVar[ErrorClass | None] = ErrorClass.CONFIG


class NothingToDo(ErError):
    """The stage had nothing to do (S4.0 exit ``10``).

    Not a failure: the ``run_stages`` row is ``status='succeeded'`` with a NULL
    ``error_class``, and a ``10`` never aborts an ``er run-all`` chain.
    """

    default_error_class: ClassVar[ErrorClass | None] = None
    unclassified_exit: ClassVar[ExitCode] = ExitCode.NOTHING_TO_DO


#: The exit statuses a caller may see. An int outside this set on some exception's
#: ``code`` attribute is a bug in the raiser, not a new status, so
#: :func:`exit_code_for` refuses to pass it through.
_KNOWN_EXITS: frozenset[int] = frozenset(int(code) for code in ExitCode)


def exit_code_for(exc: BaseException) -> int:
    """Return the S4.0 exit status for ``exc``.

    Any exception exposing an int ``code`` attribute holding a known status is
    honoured, which is the stated convention for the modules that predate this one
    (``er.config.loader.ConfigValidationError.code == 2``) and is what lets them
    stay free of an import back into the taxonomy.

    Everything else is exit ``1``. A failure the CLI cannot classify is still a
    failure, and defaulting it to ``10`` would let an unhandled exception satisfy
    the S12 M1 exit gate as a successful no-op.
    """
    code = getattr(exc, "code", None)
    # `isinstance(True, int)` is True, and SystemExit.code is commonly None or a
    # str, so the guard has to be this explicit.
    if isinstance(code, bool) or not isinstance(code, int):
        return int(ExitCode.STAGE_FAILURE)
    return code if code in _KNOWN_EXITS else int(ExitCode.STAGE_FAILURE)
