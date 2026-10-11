.DEFAULT_GOAL := check
.PHONY: run spec lint types unit dbt fixtures workflows integration check check-all clean benchmark benchmark-1m benchmark-workloads benchmark-10m frontend frontend-dev frontend-seed frontend-e2e frontend-baselines frontend-dev-reset dev-up dev-suspend dev-resume dev-down obs-up

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

# --- developer environments (infrastructure.md §12, plan-to-aws 2.1/2.2) ---
#
# Two Helm releases per developer: `$(DEV)` (er-dev-namespace, held in the
# `default` namespace because it creates $(DEV) itself) and `er-platform`
# (inside $(DEV)). The AWS side — the per-namespace EFS access point and the
# er/dev/$(DEV)/* secrets — is scripts/dev_env.sh, which needs PlatformAdmin
# credentials (see its header for the optional ER_DEV_* inputs).

ER_DEV_CONTEXT ?= er-dev

# Stand a developer environment up from nothing: make dev-up DEV=alice TAG=<git sha>
# (CI pushes er-api/er-web to ECR by commit sha; pass API_DIGEST/WEB_DIGEST to pin harder.)
dev-up:
	@test -n "$(DEV)" || { echo "usage: make dev-up DEV=<name> TAG=<git sha>"; exit 2; }
	@test -n "$(TAG)" || { echo "TAG=<git sha> required: CI pushes er-api/er-web by commit sha"; exit 2; }
	kubectl --context $(ER_DEV_CONTEXT) apply -f infra/k8s/storage/ebs-gp3.yaml
	set -e; eval "$$(bash scripts/dev_env.sh up $(DEV))"; \
	helm --kube-context $(ER_DEV_CONTEXT) upgrade --install $(DEV) infra/k8s/charts/er-dev-namespace \
	  --namespace default --set developer=$(DEV); \
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) rollout status statefulset/er-postgres --timeout=300s; \
	helm --kube-context $(ER_DEV_CONTEXT) upgrade --install er-platform infra/k8s/charts/er-platform \
	  --namespace $(DEV) --set developer=$(DEV) \
	  --set images.tag=$(TAG) \
	  $(if $(API_DIGEST),--set images.api.digest=$(API_DIGEST)) \
	  $(if $(WEB_DIGEST),--set images.web.digest=$(WEB_DIGEST)) \
	  --set efs.fileSystemId=$$ER_EFS_FS_ID --set efs.accessPointId=$$ER_EFS_AP_ID \
	  --wait --timeout 10m

# §5.2: everything to zero — Karpenter reclaims the app node, the leader
# connection goes away, Postgres stops billing compute. PVC and S3 kept.
dev-suspend:
	@test -n "$(DEV)" || { echo "usage: make dev-suspend DEV=<name>"; exit 2; }
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) scale deployment --all --replicas=0
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) scale statefulset er-postgres --replicas=0

# Back up in under a minute. kubectl, not `helm upgrade`: helm's three-way
# merge sees no manifest diff after a kubectl scale, so it would leave the
# replica counts at zero. Postgres first, then the API (its dispatcher is
# init-gated on /healthz regardless — the ordering just avoids a long wait).
dev-resume:
	@test -n "$(DEV)" || { echo "usage: make dev-resume DEV=<name>"; exit 2; }
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) scale statefulset er-postgres --replicas=1
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) rollout status statefulset/er-postgres --timeout=300s
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) scale deployment er-api er-web --replicas=1
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) rollout status deployment/er-api --timeout=300s
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) scale deployment er-dispatcher --replicas=1

# The purge path (§12): releases gone, namespace (and with it the Postgres PVC,
# hence the EBS volume) gone, then the AWS side — access point, secrets, lake
# prefix — via dev_env.sh. backend-design §7's tenant purge, environment-sized.
dev-down:
	@test -n "$(DEV)" || { echo "usage: make dev-down DEV=<name>"; exit 2; }
	helm --kube-context $(ER_DEV_CONTEXT) --namespace $(DEV) uninstall er-platform --ignore-not-found
	helm --kube-context $(ER_DEV_CONTEXT) --namespace default uninstall $(DEV) --ignore-not-found
	kubectl --context $(ER_DEV_CONTEXT) wait --for=delete namespace/$(DEV) --timeout=300s || true
	bash scripts/dev_env.sh down $(DEV)

# --- observability floor (infrastructure.md §10.5/§10.6 Phase 1, plan 1.8) ---
#
# Cluster-wide, namespace `observability`: the Fluent Bit log DaemonSet, the
# OTel collector gateway, and the OTel operator (auto-instrumentation). Chart
# versions are pinned HERE; config lives in infra/k8s/observability/*.yaml;
# the AWS side (log groups, Pod Identity roles) is
# infra/terraform/envs/dev/observability.tf. Idempotent, like dev-up.
obs-up:
	kubectl --context $(ER_DEV_CONTEXT) apply -f infra/k8s/observability/namespace.yaml
	helm repo add eks https://aws.github.io/eks-charts --force-update
	helm repo add open-telemetry https://open-telemetry.github.io/opentelemetry-helm-charts --force-update
	helm --kube-context $(ER_DEV_CONTEXT) upgrade --install fluent-bit eks/aws-for-fluent-bit \
	  --version 0.2.0 --namespace observability -f infra/k8s/observability/fluent-bit-values.yaml
	helm --kube-context $(ER_DEV_CONTEXT) upgrade --install otel-collector open-telemetry/opentelemetry-collector \
	  --version 0.175.1 --namespace observability -f infra/k8s/observability/otel-collector-values.yaml
	helm --kube-context $(ER_DEV_CONTEXT) upgrade --install otel-operator open-telemetry/opentelemetry-operator \
	  --version 0.124.1 --namespace observability -f infra/k8s/observability/otel-operator-values.yaml

# Rebuildable local caches only. Run outputs under artifacts/ are removed separately.
clean:
	rm -rf .pytest_cache .mypy_cache .ruff_cache .hypothesis dbt/target dbt/logs dbt/dbt_packages
	rm -f dbt/profiles/.user.yml
	python3 -c 'from pathlib import Path; import shutil; [shutil.rmtree(p) for root in ("src", "tests", "scripts", "benchmarks", "fixtures") for p in Path(root).rglob("__pycache__")]'
