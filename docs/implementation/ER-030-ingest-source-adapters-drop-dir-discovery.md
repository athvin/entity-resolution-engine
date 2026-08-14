---
id: ER-030
title: "Ingest source adapters: drop-dir discovery, CSV/Parquet, sources.columns projection, date_format, deterministic order"
milestone: M1
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-011", "ER-014"]
spec_refs: ["s3", "s4-0", "s4-1", "s6", "s6-1"]
gap_refs: ["M14", "M19"]
provides: ["src/er/ingest/sources.py::SourceAdapter", "src/er/ingest/sources.py::CsvAdapter", "src/er/ingest/sources.py::ParquetAdapter", "src/er/ingest/sources.py::adapter_for", "src/er/ingest/sources.py::discover_files", "src/er/ingest/sources.py::SourceRow"]
consumes: ["src/er/config/schema.py::ErConfig", "src/er/config/loader.py::load_config", "src/er/errors.py::ErrorClass", "src/er/errors.py::ExitCode", "src/er/ingest/hashing.py::content_hash", "src/er/entities/ids.py::record_key"]
owns: ["src/er/ingest/sources.py", "tests/unit/test_sources.py"]
protected_paths: []
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/test_sources.py -q && uv run mypy --strict src/er/ingest/sources.py"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

S4.1's ingest reads CSV/Parquet from `storage.drop_dir/<source>/` behind a `SourceAdapter` interface, taking the canonical→source column mapping, `record_id_column`, `updated_at_column` and `date_format` from `sources.<name>` in S6. This ticket ships that adapter layer: deterministic drop-directory discovery, row iteration in a deterministic order, projection of `sources.<name>.columns` into the ordered value tuple `content_hash` consumes, a verbatim `payload`, and the type-to-text rendering that makes a Parquet delivery hash identically to the same records delivered as CSV. Closes M14's source-schema arm and the ingest half of M19's adapter contract.

## Scope

### In scope

- `SourceAdapter` Protocol: `discover()`, `rows()` yielding `SourceRow(source_system, source_record_id, payload, values)` where `values` is the projection of `sources.<name>.columns` in declared order.
- `CsvAdapter` and `ParquetAdapter`; `adapter_for(cfg, source)` dispatching on `sources.<name>.adapter`.
- `discover_files(drop_dir, source)`: files under `<drop_dir>/<source>/` matching the adapter's extensions, returned sorted by filename; row order is file order then in-file order, and repeated invocations are identical.
- Type-to-text rendering for non-string Parquet values: date-typed columns via `sources.<name>.date_format`, timestamps as ISO-8601, everything else via `str()`, NULL as None — so CSV and Parquet deliveries of the same records project identically.
- Error mapping per S4.7/S4.0: unknown source or unknown adapter token → `config` class, exit `2`; unreadable/unparsable file, missing `record_id_column`, or a `source_record_id` containing `':'` → `data` class, exit `1`, naming the file and row.
- An empty delivery (no files, or files with zero data rows) yields zero rows without raising.

### Out of scope

- Writing anything to the lake: the anti-join append, `ingest_batches` and the five counts are ER-031.
- `--full-refresh-keys`, tombstone derivation, the empty-delivery refusal (exit `2`) and resurrection counting — ER-032 decides those from a zero-row delivery.
- `content_hash` itself (ER-029); this ticket only supplies its input tuple.
- Standardization: `parse_date`, `email_norm`, `phone_e164` and the rest of S4.2 run in dbt, not in the adapter — no value is normalized before hashing.
- Stripping `persona_id` from fixture rows: that is the fixture loader's job (S8.2), not the adapter's.

## Design decisions applied

M14 + M19. Constraints: (1) values are projected **verbatim** from the source — no trimming, no case folding, no date reformatting of string inputs — because `content_hash` is defined over the delivered source columns and normalizing here would make a reformatted redelivery hash as unchanged. (2) `date_format`'s role in this layer is exactly one thing: rendering typed Parquet date values to the same text a CSV delivery carries, so the two paths hash identically; that equality is the ticket's headline assertion. (3) `payload` holds the full source row verbatim including columns not named in `columns` — S4.1 requires it and staging extracts from it. (4) `rows()` is a generator: deliveries at benchmark scale must not be materialised. (5) Discovery order and row order are deterministic and asserted, because ingest's anti-join append and the `ingest_batches` counts are otherwise reproducible only by luck.

## Acceptance criteria

- [ ] AC1: `discover_files` returns the matching files sorted by filename, and two consecutive full iterations of `rows()` over the same directory yield byte-identical sequences of `SourceRow`.
- [ ] AC2: For a source whose `columns` block maps the nine canonical attributes of V11, `SourceRow.values` is a tuple in the declared mapping order with missing or absent source values as None, and `SourceRow.payload` contains every column present in the delivered file.
- [ ] AC3: The same ten records delivered as CSV and as Parquet produce identical `values` tuples and identical `content_hash` digests, with date-typed Parquet columns rendered through `sources.<name>.date_format`.
- [ ] AC4: An unknown source name and an unknown `adapter` token each exit `2` with the `config` error class; the failure names the offending key.
- [ ] AC5: A file whose header lacks the configured `record_id_column` raises the `data` class (exit `1`) before any row is yielded, naming the file.
- [ ] AC6: A row whose `source_record_id` contains `':'` raises the `data` class (exit `1`) naming the file and row number, per S4.7's `data` examples.
- [ ] AC7: A corrupt/unparsable file raises the `data` class (exit `1`) naming the file; a zero-file directory and a header-only file both yield zero rows and raise nothing.
- [ ] AC8: `rows()` is a generator: the first `SourceRow` is produced before the final row of a large file is read (asserted by consuming one item from a file whose tail is deliberately unparsable and observing no error).

## Tests

- tests/unit/test_sources.py::test_discovery_and_row_order_are_deterministic
- tests/unit/test_sources.py::test_projection_follows_declared_column_order
- tests/unit/test_sources.py::test_payload_is_verbatim_including_unmapped_columns
- tests/unit/test_sources.py::test_csv_and_parquet_hash_identically
- tests/unit/test_sources.py::test_unknown_source_and_adapter_exit_2
- tests/unit/test_sources.py::test_missing_record_id_column_is_data_error
- tests/unit/test_sources.py::test_colon_in_source_record_id_is_data_error
- tests/unit/test_sources.py::test_empty_delivery_yields_zero_rows
- tests/unit/test_sources.py::test_rows_is_lazy

## Verification

```bash
uv run pytest tests/unit/test_sources.py -q && uv run mypy --strict src/er/ingest/sources.py
uv run ruff check src/er/ingest/sources.py
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- CSV and Parquet paths proven to produce identical `content_hash` inputs
- Deterministic file and row order asserted, not assumed
- Error classes and exit codes match S4.0/S4.7 (`2` for config, `1` for data)
- No value normalized before hashing; `payload` verbatim
- `rows()` lazy; no full-file materialisation
- ruff + `mypy --strict src/er` clean; verify command passes
