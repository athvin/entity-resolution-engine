"""The T-INC-3 parity set: derived once, committed, and never asserted by count (S8.2.1).

S8.2.1 is normative that `parity_pairs.csv` is **DERIVED, not invented**:

    parity_pairs = { (a, b) : (a, b) is scored by the incremental two-pass path over
                              `batch/` (S4.3.4)
                            ∧ (a, b) is scored by the corpus-wide `full.py` pass over
                              base/ ∪ batch/ }

and that — because `find_matches_to_new_records` plus a batch-only `dedupe_only` linker
can only produce pairs with **at least one endpoint in `batch/`**, and because both
paths regenerate their own candidates from the same blocking rules at the same
threshold — this is exactly the set of pairs with at least one endpoint in `batch/`
that clear `review_low`.

Two consequences shape this module.

* **Cardinality is a property of the fixture, so no test asserts one.** S8.2.1 says so
  outright, and the ticket's title ("50 committed pairs") is indicative of scale only.
  :func:`derive_parity_pairs` returns a set; whether it holds 12 or 120 is the
  fixture's business. What the committed file buys is that a *silent shrink* of the
  set — a blocking regression, or a batch that stops linking — shows up as a diff
  rather than as a still-green parity test over three surviving pairs.
* **The file is regenerated deliberately and visibly.** :func:`write_parity_pairs` is
  reached only through :data:`REGEN_ENV`, and the test that calls it fails afterwards.
  A regeneration path that left the suite green would turn every parity failure into a
  self-healing no-op, which is the one way this oracle could stop being one.

The sort is S8.2.1's: byte-wise on the UTF-8 encoding of the full column tuple in
header order. `record_key` is ASCII by construction (S5.0 bans `:` in both components
and the sources are config-declared names), so the byte order and the code-point order
agree — but the encode is written out anyway, because the rule is about bytes and a
future non-ASCII source id should not silently re-sort the file.
"""

from __future__ import annotations

import csv
from collections.abc import Iterable, Mapping, Set
from pathlib import Path
from typing import Final

from er.entities.ids import canonicalize_pair

__all__ = [
    "PARITY_FILENAME",
    "PARITY_HEADER",
    "REGEN_ENV",
    "Pair",
    "derive_parity_pairs",
    "read_parity_pairs",
    "symmetric_difference_report",
    "write_parity_pairs",
]

#: One canonical pair, `rec_a_key < rec_b_key` (S5.0), as :mod:`helpers.pairs` spells it.
Pair = tuple[str, str]

#: The scenario-root file and its literal header (S8.2.1).
PARITY_FILENAME: Final = "parity_pairs.csv"
PARITY_HEADER: Final[tuple[str, ...]] = ("rec_a_key", "rec_b_key")

#: Setting this rewrites the committed file and fails the test. Not a flag, because a
#: flag is something a CI invocation can carry by accident.
REGEN_ENV: Final = "ER_REGEN_PARITY"


def derive_parity_pairs(
    incremental: Mapping[Pair, float],
    full: Mapping[Pair, float],
    batch_keys: Set[str],
) -> set[Pair]:
    """The S8.2.1 parity set: pairs both paths scored, touching `batch/`.

    The `review_low` filter is not applied here and does not need to be: neither path
    persists a pair below it (S4.3.4), so a pair present in both mappings has already
    cleared it on both. Re-filtering would let a caller pass a threshold that disagrees
    with the one the rows were written under, and the set would then be neither path's.

    The endpoint condition is asserted rather than assumed. It is a *theorem* about the
    two-pass path — `find_matches_to_new_records` pairs new against corpus, and the
    batch-only `dedupe_only` linker pairs new against new, so neither can emit a pair of
    two base records — and a violation means the incremental path scored something it
    had no candidate source for, which is a defect this set must not quietly absorb.

    Args:
        incremental: canonical pair -> probability, from the incremental universe.
        full: canonical pair -> probability, from the corpus-wide universe.
        batch_keys: the `record_key`s of the `batch/` delivery, from the fixture CSVs.

    Returns:
        The canonical pairs in both mappings that touch `batch/`.

    Raises:
        AssertionError: the incremental path scored a pair with neither endpoint in
            `batch/`.
    """
    stray = sorted(
        pair for pair in incremental if pair[0] not in batch_keys and pair[1] not in batch_keys
    )
    assert not stray, (
        f"the incremental path scored {len(stray)} pair(s) with neither endpoint in "
        f"batch/, which neither of its two passes can generate (S4.3.4): {stray[:5]}"
    )
    return {
        pair
        for pair in set(incremental) & set(full)
        if pair[0] in batch_keys or pair[1] in batch_keys
    }


def _sorted_rows(pairs: Iterable[Pair]) -> list[Pair]:
    """``pairs`` in S8.2.1 order: byte-wise on the UTF-8 column tuple, header order."""
    return sorted(pairs, key=lambda pair: (pair[0].encode("utf-8"), pair[1].encode("utf-8")))


def write_parity_pairs(path: Path, pairs: Iterable[Pair]) -> int:
    """Rewrite ``path`` with ``pairs``, byte-sorted under the literal header.

    Every pair is put back through :func:`~er.entities.ids.canonicalize_pair`, so a
    caller that assembled the set in the wrong orientation writes a file that would
    never match its own derivation — the error surfaces here rather than as a
    permanent, invisible symmetric difference.

    Args:
        path: the scenario-root `parity_pairs.csv`.
        pairs: the derived set.

    Returns:
        How many rows were written.
    """
    rows = _sorted_rows(canonicalize_pair(a, b) for a, b in pairs)
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle, lineterminator="\n")
        writer.writerow(PARITY_HEADER)
        writer.writerows(rows)
    return len(rows)


def read_parity_pairs(path: Path) -> set[Pair]:
    """The committed parity set.

    Args:
        path: the scenario-root `parity_pairs.csv`.

    Returns:
        The canonical pairs it holds.

    Raises:
        ValueError: the committed header is not :data:`PARITY_HEADER`. The file is read
            positionally, so a reordered header would silently transpose every pair.
    """
    with path.open(encoding="utf-8", newline="") as handle:
        reader = csv.reader(handle)
        header = tuple(next(reader))
        if header != PARITY_HEADER:
            raise ValueError(f"{path.name} header is {header}, expected {PARITY_HEADER} (S8.2.1)")
        return {canonicalize_pair(rec_a_key, rec_b_key) for rec_a_key, rec_b_key in reader}


def symmetric_difference_report(derived: Set[Pair], committed: Set[Pair]) -> str:
    """Both directions of the difference, which is what AC1 requires on failure.

    A count says the file drifted; the two directions say whether the parity set
    *shrank* — a blocking regression or a batch that stopped linking, which is the
    failure this file exists to make visible — or *grew*, which is usually a fixture
    edit nobody regenerated for.

    Args:
        derived: the recomputed set.
        committed: the set read from the file.

    Returns:
        A multi-line report, safe to build unconditionally as an assertion message.
    """
    missing = sorted(derived - committed)
    extra = sorted(committed - derived)
    header = (
        f"parity set (T-INC-3, S8.2.1): derived {len(derived)} pair(s), committed {len(committed)}"
    )
    if not missing and not extra:
        return f"{header}; the two sets are equal"
    lines = [header]
    lines.append(f"  derived but NOT committed ({len(missing)}) — regenerate if intended:")
    lines += [f"    {a} | {b}" for a, b in missing]
    lines.append(f"  committed but NOT derived ({len(extra)}) — the parity set SHRANK:")
    lines += [f"    {a} | {b}" for a, b in extra]
    lines.append(f"  set {REGEN_ENV}=1 to rewrite the file (the test then fails deliberately)")
    return "\n".join(lines)
