"""Unit tests for the S8.2.1 fixture format (ER-028).

A package rather than a loose directory because a sibling `tests/fixtures/` and a
`tests/unit/fixtures/` would otherwise both be importable as the top-level name
`fixtures`. This file makes the one holding tests a regular package, which
resolves ahead of the namespace package the other would form, so the module name
is stable under both pytest and mypy.
"""
