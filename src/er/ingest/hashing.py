"""``content_hash``, defined normatively in DesignDoc.md S4.1.

S4.1 pins one digest contract for both execution paths. :func:`content_hash` is
its reference/compatibility implementation, also exported as
``er.ingest.landing.content_hash``. :func:`content_hash_sql` renders the native
DuckDB expression, with file-path parity checked against the reference.

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

__all__ = ["TOMBSTONE_CONTENT_HASH", "UNIT_SEPARATOR", "content_hash", "content_hash_sql"]

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


def content_hash(
    row: Mapping[str, str | None], columns: Sequence[str], *, metadata_columns: Sequence[str] = ()
) -> str:
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

    Ingestion supplies ``metadata_columns`` for the unmapped client fields. When
    present, their sorted names and verbatim values extend the digest, so edits to
    metadata are delivered even when every standardized attribute is unchanged.
    Omitting this argument retains the original mapped-field digest.
    """
    values = []
    for column in columns:
        value = row.get(column)
        values.append("" if value is None else unicodedata.normalize("NFC", value))
    digest = hashlib.sha256(UNIT_SEPARATOR.join(values).encode("utf-8")).hexdigest()
    if not metadata_columns:
        return digest
    # Metadata is verbatim: distinguish null from empty and include field names.
    # Fixed-length name/value hashes and presence tags avoid delimiter ambiguity.
    extras = []
    for name in sorted(set(metadata_columns)):
        value = row[name]
        extras.append(hashlib.sha256(name.encode("utf-8")).hexdigest())
        extras.append(
            "N" if value is None else "V" + hashlib.sha256(value.encode("utf-8")).hexdigest()
        )
    preimage = digest + UNIT_SEPARATOR + "metadata-v1" + UNIT_SEPARATOR + "".join(extras)
    return hashlib.sha256(preimage.encode("utf-8")).hexdigest()


def content_hash_sql(
    columns: Sequence[str],
    delivered_columns: Sequence[str],
    *,
    metadata_columns: Sequence[str] = (),
) -> str:
    """The native expression for the same hash contract, over rendered VARCHARs.

    Names are quoted as identifiers, never interpolated as executable expressions.
    The reference function remains the oracle for native file-path parity tests.
    """
    values = []
    for name in columns:
        quoted = '"' + name.replace('"', '""') + '"'
        values.append(
            f"coalesce(nfc_normalize({quoted}), '')" if name in delivered_columns else "''"
        )
    digest = (
        f"sha256(concat_ws(chr({ord(UNIT_SEPARATOR)}), " + ", ".join(values) + "))"
        if values
        else "sha256('')"
    )
    if not metadata_columns:
        return digest
    extras = []
    for name in sorted(set(metadata_columns)):
        quoted = '"' + name.replace('"', '""') + '"'
        literal = "'" + name.replace("'", "''") + "'"
        extras.append(f"sha256({literal})")
        extras.append(f"CASE WHEN {quoted} IS NULL THEN 'N' ELSE 'V' || sha256({quoted}) END")
    return f"sha256({digest} || chr(31) || 'metadata-v1' || chr(31) || " + " || ".join(extras) + ")"
