"""Unit tests for the benchmark harness inputs (ER-096).

A package rather than a loose directory for the reason `tests/unit/fixtures/` is one:
the modules under test are imported by bare name off `benchmarks/`, and a regular
package here keeps this directory's own module names from colliding with them.
"""
