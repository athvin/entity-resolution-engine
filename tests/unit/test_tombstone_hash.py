"""The S4.1.1 tombstone sentinel: one definition, and unreachable from the hash.

S4.1.1 rests the whole deletion arm on one claim — "the sentinel is never produced by
the hash function above … so tombstones can never collide with content versions". If
that claim were false, a delivered row would hash to `'0' * 64` and become
indistinguishable from a tombstone: `er standardize` would drop a live record from
`int_std_records`, and the next full refresh would decline to tombstone a key it had
already lost. Nothing downstream could detect it, because by then the row *is* a
tombstone in every respect the schema can see.

So the claim is asserted two ways here, and neither is a re-reading of the comment
beside the constant:

* **Unreachable.** A property test over arbitrary rows, with the two degenerate
  shapes S4.1.1's argument turns on pinned as explicit examples: the all-NULL row and
  the all-empty-string row, which are the rows whose preimage carries no information
  at all and are therefore the closest a caller can get to hashing nothing.
* **Defined once.** The sentinel is a value, not a function, so "one implementation"
  means one assignment: `er.ingest.landing.TOMBSTONE_CONTENT_HASH` is S4.1.1's
  spelling and `er.ingest.hashing` is where S3 puts the module, and the first MUST be
  the same object as the second rather than a second `'0' * 64` that agrees today.

``tests/unit/test_content_hash.py`` states the unreachability property too, over the
hashing module alone. It is not duplicated here: that suite owns the digest, this one
owns the sentinel and the arm that writes it, and this file additionally binds the
property to the constant `landing.py` actually inserts.
"""

from __future__ import annotations

import ast
from pathlib import Path

from hypothesis import example, given
from hypothesis import strategies as st

from er.ingest import hashing, landing
from er.ingest.hashing import TOMBSTONE_CONTENT_HASH, content_hash

REPO_ROOT = Path(__file__).resolve().parents[2]

#: The name S4.1.1 gives the sentinel, and the only name it is assigned under.
SENTINEL_NAME = "TOMBSTONE_CONTENT_HASH"

#: The columns the degenerate examples are hashed over. Three, because a one-column
#: row and a many-column row differ only in how many separators the preimage carries
#: and the argument is about the preimage being non-empty either way.
DEGENERATE_COLUMNS = ("given_name", "family_name", "email")


def _assignments_to(name: str, root: Path) -> list[Path]:
    """Every module under ``root`` that binds ``name`` at any level, in path order."""
    found: list[Path] = []
    for path in sorted(root.rglob("*.py")):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        for node in ast.walk(tree):
            targets: list[ast.expr] = []
            if isinstance(node, ast.Assign):
                targets = list(node.targets)
            elif isinstance(node, ast.AnnAssign):
                targets = [node.target]
            if any(isinstance(t, ast.Name) and t.id == name for t in targets):
                found.append(path.relative_to(REPO_ROOT))
    return found


@given(
    st.dictionaries(
        keys=st.text(min_size=1, max_size=12),
        values=st.one_of(st.none(), st.text(max_size=32)),
        min_size=1,
        max_size=6,
    )
)
@example(dict.fromkeys(DEGENERATE_COLUMNS, None))
@example(dict.fromkeys(DEGENERATE_COLUMNS, ""))
def test_sentinel_is_unreachable_from_content_hash(row: dict[str, str | None]) -> None:
    """AC6: no row hashes to the sentinel, the all-NULL and all-empty rows included.

    The two ``@example`` cases are S4.1.1's own argument made concrete. A NULL and an
    empty string both encode as the empty string (S4.1), so those two rows share one
    preimage — the separators alone — and it is still a preimage, which is exactly why
    the digest cannot be the all-zero string that stands for "hashed nothing".
    """
    assert TOMBSTONE_CONTENT_HASH == "0" * 64
    assert content_hash(row, list(row)) != TOMBSTONE_CONTENT_HASH


def test_sentinel_is_defined_once() -> None:
    """AC6: one assignment in ``src/er``, and `landing.py` re-exports that object.

    Identity, not equality: a second ``'0' * 64`` elsewhere would compare equal to
    this one forever and would still be the drift S4.1 forbids for the digest and
    S4.1.1 needs for the sentinel — the tombstone the writer inserts and the value a
    reader filters on have to be the same decision, not two that currently agree.
    """
    assert _assignments_to(SENTINEL_NAME, REPO_ROOT / "src" / "er") == [
        Path("src/er/ingest/hashing.py")
    ]
    assert landing.TOMBSTONE_CONTENT_HASH is hashing.TOMBSTONE_CONTENT_HASH
    assert SENTINEL_NAME in landing.__all__

    # The value the tombstone statement actually writes, rather than a constant that
    # merely sits beside it: a statement that inlined its own literal would pass every
    # assertion above and still insert something else.
    assert "?" in landing._APPEND_TOMBSTONES
    assert landing.TOMBSTONE_CONTENT_HASH not in landing._APPEND_TOMBSTONES
