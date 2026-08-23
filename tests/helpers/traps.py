"""`base_10`'s ground truth, derived once: the trap index and the true pair set (S8.2.1).

S8.2.1 puts `truth.csv` and `traps.csv` in the *ground truth* half of the scenario root —
read only by tests and by the S8.5 metrics, never fed to the pipeline. This module is the
one place either is parsed, for the reason :mod:`helpers.pairs` gives about the two
candidate pair sets: a second reader drifts from the first, and two readers of the truth
would let a trap dissolve on one side of a comparison while the other still believed in
it.

Two properties of the derivation are load-bearing.

* **Everything is expressed in `record_key`.** `truth.csv` and `traps.csv` are keyed by
  `(source_system, source_record_id)` because that is what a fixture author can write by
  hand, but every relation a trap assertion reads — `int_std_records`, `match_scores`,
  `int_blocking_keys` — is keyed by the canonical scalar identity of S5.0. Converting
  here rather than at each call site means :func:`~er.entities.ids.record_key` validates
  every committed row once, so a `:` smuggled into a fixture id fails as a key error with
  the offending row named instead of as an empty join downstream.
* **Pairs come back canonical.** :func:`true_pairs_from_truth` returns
  `rec_a_key < rec_b_key` pairs built through
  :func:`~er.entities.ids.canonicalize_pair`, which is the same orientation
  `match_scores` and :mod:`helpers.pairs` use. A truth set in the other orientation would
  make every membership test silently false rather than loudly wrong.

`traps.csv` is the machine-readable form of the S8.2 designed-traps table: one row per
`(trap, source_system, source_record_id)`, so a trap is exactly the set of records that
construct it. That is what makes the completeness check of
`tests/integration/test_base_10_traps.py` possible at all — the asserted trap ids are
compared against this index rather than against a list re-typed from the spec, so adding
a row to the S8.2 table and its fixture index fails the suite until an assertion exists.
"""

from __future__ import annotations

import csv
from collections.abc import Iterator, Mapping
from pathlib import Path
from typing import Final

from er.entities.ids import canonicalize_pair, record_key

__all__ = [
    "TRAPS_HEADER",
    "TRUTH_HEADER",
    "Pair",
    "load_trap_index",
    "persona_members",
    "persona_of_record",
    "true_pairs_from_truth",
]

#: One canonical pair, `rec_a_key < rec_b_key` (S5.0), matching :mod:`helpers.pairs`.
Pair = tuple[str, str]

#: The two committed headers, literal. Both files are read positionally below, so a
#: reordered header would load a `source_record_id` into the persona label.
TRUTH_HEADER: Final[tuple[str, ...]] = ("persona_id", "source_system", "source_record_id")
TRAPS_HEADER: Final[tuple[str, ...]] = ("trap", "source_system", "source_record_id")


def _labelled_rows(path: Path, header: tuple[str, ...]) -> Iterator[tuple[str, str]]:
    """Yield `(label, record_key)` for a three-column ground-truth file.

    Both `truth.csv` and `traps.csv` have the same shape — a label followed by the two
    components of a record identity — so they have one reader.

    Args:
        path: the committed CSV.
        header: the literal header the file must carry.

    Yields:
        The label of each row and the S5.0 `record_key` its two id columns denote.

    Raises:
        ValueError: the committed header is not ``header``.
    """
    with path.open(encoding="utf-8", newline="") as handle:
        reader = csv.reader(handle)
        found = tuple(next(reader))
        if found != header:
            raise ValueError(f"{path.name} header is {found}, expected {header} (S8.2.1)")
        for label, source_system, source_record_id in reader:
            yield label, record_key(source_system, source_record_id)


def _grouped(path: Path, header: tuple[str, ...]) -> dict[str, tuple[str, ...]]:
    """Label -> the record keys carrying it, each label's keys sorted and distinct."""
    groups: dict[str, set[str]] = {}
    for label, key in _labelled_rows(path, header):
        groups.setdefault(label, set()).add(key)
    return {label: tuple(sorted(keys)) for label, keys in sorted(groups.items())}


def load_trap_index(path: Path) -> Mapping[str, tuple[str, ...]]:
    """The S8.2 designed traps, as trap name -> the record keys that construct it.

    Args:
        path: the scenario's committed `traps.csv`.

    Returns:
        Trap name -> its record keys, sorted. The key set of the mapping is the S8.2
        trap table, which is what the completeness assertion compares against.

    Raises:
        ValueError: the committed header is not :data:`TRAPS_HEADER`.
    """
    return _grouped(path, TRAPS_HEADER)


def persona_members(path: Path) -> Mapping[str, tuple[str, ...]]:
    """The ground-truth partition: persona label -> its record keys.

    This is the only membership a trap assertion may read. `entity_membership` is the
    pipeline's *answer* and does not exist before M4; asserting a designed trap against
    it would be asserting the pipeline against itself.

    Args:
        path: the scenario's committed `truth.csv`.

    Returns:
        Persona id -> its record keys, sorted.

    Raises:
        ValueError: the committed header is not :data:`TRUTH_HEADER`.
    """
    return _grouped(path, TRUTH_HEADER)


def persona_of_record(path: Path) -> Mapping[str, str]:
    """The inverse of :func:`persona_members`: record key -> its persona label.

    Args:
        path: the scenario's committed `truth.csv`.

    Returns:
        Record key -> persona id.

    Raises:
        ValueError: the committed header is not :data:`TRUTH_HEADER`, or one record key
            carries two persona labels — a truth file that labels a record twice makes
            precision and recall both undefined.
    """
    labels: dict[str, str] = {}
    for persona, key in _labelled_rows(path, TRUTH_HEADER):
        existing = labels.setdefault(key, persona)
        if existing != persona:
            raise ValueError(f"{key} is labelled both {existing} and {persona} in {path.name}")
    return labels


def true_pairs_from_truth(path: Path) -> set[Pair]:
    """Every same-persona pair of `truth.csv`, canonical (S8.2: 18 of them on `base_10`).

    This is the recall denominator of S8.5 and the set every quality trap is stated
    over. It is derived rather than committed because a committed pair list is a second
    encoding of the persona labels, and the two would disagree the first time a record
    moved persona.

    Args:
        path: the scenario's committed `truth.csv`.

    Returns:
        The canonical pairs, each exactly once.

    Raises:
        ValueError: the committed header is not :data:`TRUTH_HEADER`.
    """
    pairs: set[Pair] = set()
    for keys in persona_members(path).values():
        for index, left in enumerate(keys):
            for right in keys[index + 1 :]:
                pairs.add(canonicalize_pair(left, right))
    return pairs
