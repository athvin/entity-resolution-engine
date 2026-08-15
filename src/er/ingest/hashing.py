"""``content_hash``, defined normatively in DesignDoc.md S4.1.

S4.1 pins the digest and then pins its *cardinality*: it is "computed by exactly one
function", because two implementations that disagree make T-IDEM-1a and T-IDEM-1
non-reproducible. That one function is :func:`content_hash` here. S4.1 spells its
import path ``er.ingest.landing.content_hash`` while the S3 tree puts the file at
``ingest/hashing.py``; ER-031 re-exports this object from ``landing.py`` so both
spellings resolve to the same function. A second implementation behind the other
name is the defect S4.1 forbids.

The module imports only ``hashlib``, ``unicodedata`` and ``collections.abc`` — not
even ``__future__``, which is asserted rather than merely intended (see
``tests/unit/test_content_hash.py::test_module_imports_only_stdlib``). The fixture
linter, the synthetic generator and the dbt-side oracle all need this digest, and a
dependency on config or lake code would put a connection or a Pydantic model in
their import path.
"""

import hashlib
import unicodedata
from collections.abc import Mapping, Sequence

__all__ = ["TOMBSTONE_CONTENT_HASH", "UNIT_SEPARATOR", "content_hash"]

#: S4.1: source values are joined by the ``0x1f`` unit separator. Values are hashed
#: verbatim, so a value that itself contains ``0x1f`` is ambiguous with a column
#: boundary. That is a property of the pinned encoding, not an oversight — the
#: committed vectors record the colliding pair explicitly rather than escaping it.
UNIT_SEPARATOR = "\x1f"

#: S4.1.1: a tombstone version row carries this sentinel and ``payload = NULL``.
#: :func:`content_hash` cannot produce it — S4.1.1's argument is that the function
#: always hashes at least one separator-joined value, and no SHA-256 digest of any
#: such preimage is known to be all-zeroes. The hypothesis property in the test
#: module is the standing proof; this comment is not.
TOMBSTONE_CONTENT_HASH = "0" * 64


def content_hash(row: Mapping[str, str | None], columns: Sequence[str]) -> str:
    """Return the S4.1 ``content_hash`` of ``row`` over ``columns``.

    ``columns`` is the source column names from ``sources.<name>.columns`` (S6) **in
    the declared order**; projecting the config mapping into that order belongs to
    the adapter (ER-030), not here. Each value is NFC-normalized, NULL encodes as the
    empty string, the values are joined by :data:`UNIT_SEPARATOR` and the UTF-8
    concatenation is SHA-256'd to lowercase hex.

    Two consequences are by design and are asserted, not worked around:

    * A NULL and an empty string in the same position hash identically — both encode
      as the empty string.
    * ``ingested_at``, ``ingest_batch_id`` and ``std_version`` are excluded by
      construction: they are never members of ``columns``, so extra keys in ``row``
      cannot reach the preimage.

    A column absent from ``row`` is treated as NULL, so a source row missing an
    optional column hashes the same as one carrying it empty.
    """
    values = []
    for column in columns:
        value = row.get(column)
        values.append("" if value is None else unicodedata.normalize("NFC", value))
    return hashlib.sha256(UNIT_SEPARATOR.join(values).encode("utf-8")).hexdigest()
