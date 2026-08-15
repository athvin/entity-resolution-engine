"""The S5.2 structured log line: one JSON object per stage, on stderr.

S5.2 is explicit about all three of the things this module holds fixed:

* **Exactly one line per stage**, emitted on completion, including on failure with
  ``status:"failed"`` and a populated ``error_class``/``error_detail``.
* **A fixed key set**, given by the S5.2 example line. :data:`STAGE_RECORD_KEYS` is
  that key set in that order, and :func:`emit_stage_record` refuses a record that
  is missing a key or carries an extra one — a telemetry consumer parses by key,
  and a stage that quietly dropped ``snapshot_end`` would look like a stage that
  never committed anything.
* **stderr, always.** stdout is reserved for command output so a caller can pipe it
  without parsing telemetry, and ``--json`` switches *stdout* and never moves this
  line (S4 preamble).

One key is here that S5.2's example line does not list: ``exit_code``. S4.0 makes
exit ``10`` a *successful* no-op, so ``status`` alone cannot distinguish a stage
that did its work from one that found none, and ER-014 put the code on the record
for exactly that reason. It is a superset of the S5.2 example, never a substitute:
every key of the example is present, in the example's order.
"""

from __future__ import annotations

import json
import sys
from collections.abc import Mapping
from typing import Final, TextIO

from er.lake.model import PROMOTED_COUNTERS

__all__ = ["STAGE_RECORD_KEYS", "emit_stage_record"]

#: The S5.2 example line's keys, in its order, with ``exit_code`` after ``status``.
#: The eleven promoted counters come from the registry tuple rather than being
#: retyped: S5.2's example lists them in exactly that order, and a hand-copied
#: second spelling is how the log line and the columns stop agreeing.
STAGE_RECORD_KEYS: Final[tuple[str, ...]] = (
    "run_id",
    "stage",
    "seq",
    "status",
    "exit_code",
    "started_at",
    "ended_at",
    "snapshot_start",
    "snapshot_end",
    "config_hash",
    "model_version",
    "tf_snapshot_id",
    *PROMOTED_COUNTERS,
    "error_class",
    "error_detail",
)


def emit_stage_record(record: Mapping[str, object], *, stream: TextIO | None = None) -> None:
    """Write ``record`` as exactly one JSON line on stderr (S4 preamble, S5.2).

    THE emitter. Every stage record in the process goes through it, because "exactly
    one line per stage" is only checkable when there is one place that writes one.

    Args:
        record: the stage record; its key set MUST equal :data:`STAGE_RECORD_KEYS`.
        stream: where to write; ``sys.stderr`` by default. Overridden by tests and
            by nothing else — S5.2 puts this line on stderr, full stop.

    Raises:
        ValueError: the key set is not :data:`STAGE_RECORD_KEYS`, naming what is
            missing and what is extra.
    """
    missing = [key for key in STAGE_RECORD_KEYS if key not in record]
    extra = [key for key in record if key not in STAGE_RECORD_KEYS]
    if missing or extra:
        raise ValueError(f"stage record key set is not S5.2's: missing={missing}, extra={extra}")
    # Rebuilt in STAGE_RECORD_KEYS order rather than dumped as given: the order is
    # part of the contract the ticket states, and a caller's dict order is not.
    ordered = {key: record[key] for key in STAGE_RECORD_KEYS}
    target = sys.stderr if stream is None else stream
    target.write(json.dumps(ordered, separators=(",", ":"), ensure_ascii=False) + "\n")
    target.flush()
