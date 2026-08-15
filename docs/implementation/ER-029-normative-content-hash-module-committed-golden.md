---
id: ER-029
title: "Normative content_hash module + committed golden vector (pure-Python oracle)"
milestone: M1
status: done
kind: code
size: S
gates: fast
depends_on: ["ER-011"]
spec_refs: ["s4-1", "s4-1-1", "s5", "s6", "s12-1"]
gap_refs: ["M14", "M15", "D7"]
provides: ["src/er/ingest/hashing.py::content_hash", "src/er/ingest/hashing.py::TOMBSTONE_CONTENT_HASH", "src/er/ingest/hashing.py::UNIT_SEPARATOR", "tests/fixtures/content_hash_vectors.json"]
consumes: ["src/er/config/schema.py::ErConfig"]
owns: ["src/er/ingest/hashing.py", "tests/unit/test_content_hash.py", "tests/fixtures/content_hash_vectors.json"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/test_content_hash.py -q && uv run mypy --strict src/er/ingest/hashing.py"
branch: "ticket/ER-029-normative-content-hash-module-committed-golden"
commit: "b58feb0fa8b92e69d33209669595cd0c990dc9b4"
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T03:20:08Z"
session: 039fd346-da14-4b11-a7a8-370943b16dfc
---
## Description

Two implementations that disagree on `content_hash` make T-IDEM-1a and T-IDEM-1 non-reproducible, which is why S4.1 pins it normatively and confines it to exactly one function. This ticket ships that function — SHA-256, lowercase hex, over the UTF-8 concatenation of the source columns named in `sources.<name>.columns` in the declared order, each value NFC-normalized, joined by `0x1f`, NULL encoded as the empty string — together with the `'0' * 64` tombstone sentinel and a committed golden vector file whose digests are verified against a written-out preimage rather than against the module itself. Closes M14's hashing arm, the sentinel arm of M15/D8, and pins D7.

## Scope

### In scope

- `content_hash(row, columns) -> str`: lowercase hex SHA-256 over the `0x1f`-joined, NFC-normalized values of `columns` in declared order, NULL → empty string.
- `TOMBSTONE_CONTENT_HASH = '0' * 64` plus the proof obligation that the hash function can never produce it.
- `tests/fixtures/content_hash_vectors.json`: at least eight committed vectors, each carrying `columns`, `row`, `preimage_hex` and `digest`.
- A pure-Python oracle in the test that recomputes each digest with `hashlib.sha256(bytes.fromhex(preimage_hex))`, so the vectors are not merely a recording of the module's current behaviour.
- Vectors covering: plain ASCII; NFD vs NFC equivalence; a value containing `0x1f`; empty string and NULL collapsing to the same encoding; column-order sensitivity; a single-column row; a row with unmapped extra keys; a multi-byte non-Latin value.
- A hypothesis property asserting the digest never equals the sentinel.

### Out of scope

- The anti-join append into `raw_records`, the `ingest_batches` manifest and the counters (ER-031).
- Tombstone derivation, `--full-refresh-keys`, the empty-delivery guard and resurrection (ER-032) — only the sentinel constant and its non-collision proof ship here.
- Reading source files or projecting `sources.<name>.columns` into values (ER-030).
- `std_hash` / `table_content_hash` over `int_std_records` (T-STD-1, ER-044) — a different digest over a different column list.

## Design decisions applied

M14 + M15 + D7. Constraints: (1) S4.1 names the callable as `er.ingest.landing.content_hash(row, columns)` while the S3 tree puts the implementation in `ingest/hashing.py`; the implementation lives in `hashing.py` (the verify command types that file) and **ER-031 must re-export it from `landing.py` so both spec paths resolve to one object** — a second implementation is exactly the defect S4.1 forbids. (2) NULL and the empty string both encode as the empty string and therefore hash identically; that collision is by design and is asserted rather than worked around. (3) The hash function always joins at least one value with the separator, which is why the all-zeroes sentinel is unreachable — the property test is the proof, not a comment. (4) `ingested_at`, `ingest_batch_id` and `std_version` are excluded by construction: they are never members of `columns`. (5) The module is dependency-free (stdlib only) so the fixture linter, the generator and the dbt-side oracle can all import it without pulling in config or lake code.

## Acceptance criteria

- [ ] AC1: For every committed vector, `hashlib.sha256(bytes.fromhex(preimage_hex)).hexdigest() == vector['digest'] == content_hash(vector['row'], vector['columns'])` — all three equal, so the file is an independent oracle rather than a recording.
- [ ] AC2: The NFD and NFC spellings of the same value hash identically, and the vector file contains that pair with distinct `row` bytes and one shared digest.
- [ ] AC3: A NULL value and an empty-string value in the same position produce the same digest, asserted explicitly as designed behaviour.
- [ ] AC4: Swapping two entries in `columns` changes the digest for a row whose two values differ, proving declared order is load-bearing.
- [ ] AC5: Adding `ingested_at`, `ingest_batch_id` or `std_version` keys to `row` without adding them to `columns` leaves the digest unchanged.
- [ ] AC6: `TOMBSTONE_CONTENT_HASH == '0' * 64`, and a hypothesis run over arbitrary non-empty column lists and row values never produces it.
- [ ] AC7: `src/er/ingest/hashing.py`'s top-level imports are a subset of `{hashlib, unicodedata, typing, collections.abc}` — asserted by parsing the module, so the function stays importable from the fixture and generator layers.
- [ ] AC8: A value containing a literal `0x1f` byte is hashed verbatim and its vector's `preimage_hex` shows the separator ambiguity explicitly (documented, not silently escaped).

## Tests

- tests/unit/test_content_hash.py::test_every_committed_vector_matches_independent_oracle
- tests/unit/test_content_hash.py::test_nfc_normalization_collapses_nfd
- tests/unit/test_content_hash.py::test_null_and_empty_string_share_an_encoding
- tests/unit/test_content_hash.py::test_column_order_is_load_bearing
- tests/unit/test_content_hash.py::test_excluded_fields_do_not_affect_digest
- tests/unit/test_content_hash.py::test_tombstone_sentinel_is_unreachable
- tests/unit/test_content_hash.py::test_module_imports_only_stdlib

## Verification

```bash
uv run pytest tests/unit/test_content_hash.py -q && uv run mypy --strict src/er/ingest/hashing.py
uv run ruff check src/er/ingest/hashing.py
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- Exactly one `content_hash` implementation in the repository
- Golden vector file committed with independently computable preimages
- Tombstone sentinel constant exported and proven unreachable
- Module stdlib-only and `mypy --strict` clean
- Re-export requirement for `er.ingest.landing.content_hash` recorded for ER-031
- verify command passes
