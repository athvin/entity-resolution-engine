# Developer entry points for the gate ladder.
#
# scripts/gates.sh is the single source of truth for what each gate runs; it is a
# protected path and this file restates its strings rather than inventing its own.
# tests/unit/test_package_layout.py asserts every recipe below is byte-equal to
# the line `bash scripts/gates.sh --list` prints for the gate of the same name, so
# the two cannot drift.

.PHONY: spec board lint types unit dbt gates

spec:
	python3 scripts/lint_spec.py DesignDoc.md

board:
	python3 scripts/board.py validate

lint:
	uv run ruff check . && uv run ruff format --check .

types:
	uv run mypy --strict src/er

unit:
	uv run pytest tests/unit -q --maxfail=5

dbt:
	uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem

# `gates` names no gate: it runs the whole ladder, including the Compose
# integration suite, through the script that owns the ordering.
gates:
	bash scripts/gates.sh --scope full
