"""Unit tests for the Python standardization oracles (`src/er/std/`).

This package exists so `address_cases` has ONE module name. The case table is
shared by `tests/unit/std/test_address_parser.py` and `tests/unit/dbt/test_address.py`
-- the SQL macro and the Python oracle are compared against the same inputs, which
is the whole point of committing it -- and pytest's rootdir-based collection would
otherwise put it on `sys.path` only once the `std` directory happened to be
collected. `pythonpath = ["tests"]` (pyproject.toml) plus this file makes
`unit.std.address_cases` importable from either file, in either collection order.
"""
