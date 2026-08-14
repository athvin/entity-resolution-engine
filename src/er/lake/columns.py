"""The three column sets S5.0 requires to have exactly one definition.

Determinism comparisons, config validation (S6.1 V2/V6), golden assembly and the
fixture helpers of S8.2.1 all need these lists, and they sit in distant packages.
Listing them twice is how the two copies drift, so they are listed here and
imported everywhere else — ``tests/helpers/compare.py`` in particular imports
:data:`VOLATILE_COLUMNS` rather than carrying its own exclusion list (S5.0, M7).

The module imports the stdlib only, and only ``typing``. Every package above it
consumes it — ``config``, ``lake``, ``golden``, ``tests/helpers`` — so a single
import of ``er.<anything>`` here would close an import cycle for all of them. The
guard is asserted by ``tests/unit/test_columns.py``, not left to reviewer habit.

Both tuples are ordered, and the order is the S5 DDL order. It is load-bearing:
downstream projections and comparisons read them positionally, so neither may be
sorted, and neither may be turned into a set for convenience.
"""

from typing import Final

__all__ = [
    "GOLDEN_SURVIVABLE_COLUMNS",
    "STD_RECORD_COLUMNS",
    "VOLATILE_COLUMNS",
]

# The nine columns excluded from every determinism comparison: content hashes,
# `expected/` diffs, T-STD-1, T-INC-1, T-IDEM-1 (S5.0). They are the run-scoped
# stamps — batch, timestamps, ids and the `run_stages` sequence — that differ
# between two runs which produced identical data, which is exactly why comparing
# them would make every determinism test fail for the wrong reason.
VOLATILE_COLUMNS: Final[frozenset[str]] = frozenset(
    {
        "ingest_batch_id",
        "ingested_at",
        "assembled_at",
        "scored_at",
        "assigned_at",
        "occurred_at",
        "run_id",
        "event_id",
        "seq",
    }
)

# The eleven `golden_records` columns produced by survivorship, in S5 DDL order:
# every column except `entity_id` (the key) and `survivorship_version` /
# `assembled_at` (provenance stamps, not survivable attributes). `email_valid` and
# `phone_valid` are deliberately absent — they live on `int_std_records` as inputs
# to the `validated` rule (S6.1 V4) and are not columns of `golden_records`.
# S6.1 V2 keeps this set-equal to the `survivorship:` key set with `address`
# expanded to the six `addr_*` columns; S4.6 requires those six to come from one
# winning record, so their relative order here is that record's column order.
GOLDEN_SURVIVABLE_COLUMNS: Final[tuple[str, ...]] = (
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
)

# The `int_std_records` column list of S5, in DDL order. S6.1 V6 validates every
# column named by `blocking[].expr` and every key of `comparisons` against it, and
# T-STD-1's `std_hash` projection (ER-044) is a subset of it. `record_key` leads
# because it is the relation's logical key and Splink's `unique_id_column_name`
# (S5.0, D6). The two `VOLATILE_COLUMNS` members that appear here — the ingest
# stamps — trail the payload, as they do in the DDL.
STD_RECORD_COLUMNS: Final[tuple[str, ...]] = (
    "record_key",
    "source_system",
    "source_record_id",
    "content_hash",
    "std_version",
    "given_name",
    "family_name",
    "name_variants",
    "email",
    "email_valid",
    "phone_e164",
    "phone_valid",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "updated_at_source",
    "ingest_batch_id",
    "ingested_at",
)
