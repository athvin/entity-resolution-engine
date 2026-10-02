.DEFAULT_GOAL := check
.PHONY: run spec lint types unit dbt fixtures workflows integration check check-all clean benchmark benchmark-1m benchmark-workloads benchmark-10m frontend frontend-dev frontend-seed frontend-e2e frontend-baselines frontend-dev-reset

BENCHMARK_REPEAT ?= 1

.PHONY: profile-workloads
profile-workloads:
	uv run python benchmarks/full_pipeline.py --scale 1m --local --repeat $(BENCHMARK_REPEAT) --config configs/default.yaml --with-incremental --incremental-records 100000 --incremental-scenario mixed-v1 --profile --with-profile-control

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

benchmark: benchmark-1m

benchmark-1m:
	uv run python benchmarks/full_pipeline.py --scale 1m --local --repeat $(BENCHMARK_REPEAT) --config configs/default.yaml

benchmark-workloads:
	uv run python benchmarks/full_pipeline.py --scale 1m --local --repeat $(BENCHMARK_REPEAT) --config configs/default.yaml --with-incremental --with-correction

benchmark-10m:
	uv run python benchmarks/full_pipeline.py --scale 10m --local --repeat $(BENCHMARK_REPEAT) --config configs/default.yaml

# --- frontend (standalone pnpm project; like server/, absent from the default check) ---

# The PR-equivalent gate: lint + format + types + unit, then the mock-backed
# Playwright tier (visual baselines compare on Linux only; see frontend/README.md).
frontend:
	cd frontend && corepack pnpm install --frozen-lockfile && corepack pnpm check && corepack pnpm build && corepack pnpm test:e2e

# Containers (Postgres x2 + MinIO) + erweb migrations + erserver api/dispatcher; blocks.
run:
	bash frontend/dev/run.sh

frontend-dev:
	bash frontend/dev/up.sh

# One resolved tenant (acme-dev) with corpus, model, and full run; ~3-4 minutes.
frontend-seed:
	uv run --project server --extra test python frontend/dev/seed.py

# The full-stack release gate: real browser against the seeded dev stack.
frontend-e2e:
	cd frontend && FRONTEND_E2E=1 corepack pnpm test:e2e:full

# Regenerate visual baselines inside the pinned Playwright Linux image so they
# are stable across contributor machines.
frontend-baselines:
	bash frontend/dev/update-baselines.sh

frontend-dev-reset:
	docker compose -f frontend/dev/compose.yaml down -v
	rm -rf frontend/dev/.state

# Rebuildable local caches only. Run outputs under artifacts/ are removed separately.
clean:
	rm -rf .pytest_cache .mypy_cache .ruff_cache .hypothesis dbt/target dbt/logs dbt/dbt_packages
	rm -f dbt/profiles/.user.yml
	python3 -c 'from pathlib import Path; import shutil; [shutil.rmtree(p) for root in ("src", "tests", "scripts", "benchmarks", "fixtures") for p in Path(root).rglob("__pycache__")]'
