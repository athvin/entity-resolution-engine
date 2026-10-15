.DEFAULT_GOAL := check
.PHONY: build fmt lint test check check-e2e frontend image-api dev-up dev-suspend dev-resume dev-down obs-up clean

# --- Rust backend: the application (engine, CLI, control plane) lives in rust/ ---
#
# The Python/dbt implementation has been removed; the backend is the Rust
# workspace under rust/. CI is .github/workflows/rust.yml.

build:
	cd rust && cargo build --workspace

fmt:
	cd rust && cargo fmt --all --check

lint:
	cd rust && cargo clippy --workspace --all-targets -- -D warnings

test:
	cd rust && cargo test --workspace

# The fast PR-equivalent backend gate (mirrors rust.yml's lint + test tiers).
check: fmt lint test

# The gated e2e tiers: the DuckLake + extension pipeline and the Postgres
# control-plane queue/dispatch. Needs the DuckDB extensions and a local
# Postgres (see rust.yml's ducklake-e2e job for the service definition).
# Point ER_TEST_PG_DSN at your Postgres, e.g.
#   make check-e2e ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er'
check-e2e:
	cd rust && DEDUPE_E2E_DUCKLAKE=1 DEDUPE_E2E_EXTENSIONS=1 ER_TEST_PG_DSN="$(ER_TEST_PG_DSN)" \
	  cargo test -p dedupe-cli -p er-cli -p er-server

# --- frontend (standalone pnpm project; keeps its own gate) ---

# The PR-equivalent gate: lint + format + types + unit, then the mock-backed
# Playwright tier (visual baselines compare on Linux only; see frontend/README.md).
frontend:
	cd frontend && corepack pnpm install --frozen-lockfile && corepack pnpm check && corepack pnpm build && corepack pnpm test:e2e

# --- container images (replaces the deleted ci.yaml image build) ---
#
# Build + push the Rust control-plane image to ECR by git sha. The one image
# serves er-api (the server), er-dispatcher (CMD override), and the `er`
# pipeline CLI — see rust/Dockerfile. Then `make dev-up TAG=<sha>` rolls it out.
# Needs AWS creds with ECR push. (er-web is built from frontend/Dockerfile by
# the frontend pipeline.)
ECR_REGISTRY ?= 797781631727.dkr.ecr.us-east-2.amazonaws.com
ECR_REGION ?= us-east-2

# --platform linux/amd64: the EKS nodes are amd64, so pin it (a build on an
# arm64 workstation would otherwise push an unpullable image). buildx builds +
# pushes in one step.
image-api:
	@test -n "$(TAG)" || { echo "usage: make image-api TAG=<git sha>"; exit 2; }
	aws ecr get-login-password --region $(ECR_REGION) \
	  | docker login --username AWS --password-stdin $(ECR_REGISTRY)
	docker buildx build --platform linux/amd64 \
	  -t $(ECR_REGISTRY)/er-api:$(TAG) -f rust/Dockerfile rust --push

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
	  --namespace default --set developer=$(DEV) --wait; \
	helm --kube-context $(ER_DEV_CONTEXT) upgrade --install er-platform infra/k8s/charts/er-platform \
	  --namespace $(DEV) --set developer=$(DEV) \
	  --set images.tag=$(TAG) \
	  $(if $(API_DIGEST),--set images.api.digest=$(API_DIGEST)) \
	  $(if $(WEB_DIGEST),--set images.web.digest=$(WEB_DIGEST)) \
	  --set efs.fileSystemId=$$ER_EFS_FS_ID --set efs.accessPointId=$$ER_EFS_AP_ID \
	  --wait --timeout 10m

# §5.2: scale the workload to zero — Karpenter reclaims the app node. The
# control-plane DB is Aurora now, which auto-pauses to 0 ACU on its own once the
# last connection drops, so there is no StatefulSet to scale. S3 + the DB kept.
dev-suspend:
	@test -n "$(DEV)" || { echo "usage: make dev-suspend DEV=<name>"; exit 2; }
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) scale deployment --all --replicas=0

# Back up in under a minute. kubectl, not `helm upgrade`: helm's three-way
# merge sees no manifest diff after a kubectl scale, so it would leave the
# replica counts at zero. One merged web-api deployment (API + in-process
# dispatcher + BFF); Aurora resumes from 0 ACU on the first connection (~15s).
dev-resume:
	@test -n "$(DEV)" || { echo "usage: make dev-resume DEV=<name>"; exit 2; }
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) scale deployment web-api --replicas=1
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) rollout status deployment/web-api --timeout=300s

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

# Rebuildable local caches only.
clean:
	cd rust && cargo clean
