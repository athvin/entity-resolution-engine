.DEFAULT_GOAL := check
.PHONY: spec lint types unit dbt fixtures workflows integration check check-all clean benchmark-1m benchmark-10m

spec:
	uv run python scripts/lint_spec.py DesignDoc.md

lint:
	uv run ruff check .
	uv run ruff format --check .

types:
	uv run mypy --strict src/er

unit:
	uv run pytest tests/unit -q --maxfail=5

dbt:
	uv run dbt deps --project-dir dbt
	uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem

fixtures:
	uv run python scripts/validate_fixtures.py

workflows:
	bash scripts/ci/actionlint.sh

integration:
	COMPOSE_PROJECT_NAME=er-integration bash scripts/ci/itest.sh tests/integration -q -m 'not slow'

check:
	$(MAKE) spec lint types workflows fixtures dbt unit

check-all: check
	$(MAKE) integration

benchmark-1m:
	uv run python benchmarks/full_pipeline.py --scale 1m --local

benchmark-10m:
	uv run python benchmarks/full_pipeline.py --scale 10m --local

# Rebuildable local caches only. Run outputs under artifacts/ are removed separately.
clean:
	rm -rf .pytest_cache .mypy_cache .ruff_cache .hypothesis dbt/target dbt/logs dbt/dbt_packages
	rm -f dbt/profiles/.user.yml
	python3 -c 'from pathlib import Path; import shutil; [shutil.rmtree(p) for root in ("src", "tests", "scripts", "benchmarks", "fixtures") for p in Path(root).rglob("__pycache__")]'
