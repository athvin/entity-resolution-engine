.DEFAULT_GOAL := check
.PHONY: build fmt lint doc test check check-e2e entities-check skills-check scripts-home-check workflow-shell-check tf-static-credentials-check frontend frontend-audit frontend-coverage run frontend-dev frontend-e2e frontend-baselines frontend-dev-reset image-api cluster-objects nodepools kind-up kind-down check-kind measure-runner-startup dev-up dev-suspend dev-resume dev-down obs-up clean symposium

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

# rustdoc with warnings denied — the same step rust.yml's `fmt + clippy + docs`
# job runs. It was missing here, so a public doc linking to a private item
# passed `make check` and failed CI; the one thing this target promises is that
# it does not do that.
doc:
	cd rust && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items

# Plain `cargo test`, matching CI. `cargo nextest` was measured here and is
# SLOWER for this workspace: it runs each test in its own process, and with 586
# tests across ~147 binaries that each statically link bundled DuckDB, the
# spawn-and-load cost beats the extra overlap (A/B on CI's `test` job, same
# cache state: cargo 357s vs nextest 453s). It does win on the Postgres tier,
# where tests are few and slow — but the way to collect that here is fewer,
# larger test binaries, not a different runner. See rust.yml's `test` job.
test:
	cd rust && cargo test --workspace

# The entity-framework gate: regenerate every artifact from entities/*/entity.yaml and
# fail if any committed file is stale (run `er-entity gen` to fix), then plan every
# statement and run the E0xx passes. This is what rust.yml runs as `er-entity verify`.
entities-check:
	cd rust && cargo run -q -p er-entity-cli -- verify --root ..

# The agent-skill gate (.claude/skills/**). Two modes, split by what their
# output derives from: `lint` validates the skills against the Agent Skills
# authoring rules and needs no toolchain, while `regen` rebuilds the GENERATED
# blocks inside skill references from entities/, the crate tree, analyze.rs, the
# workflows and the frontend config, then fails on drift. See
# .claude/skills/skill-writer/references/agent-skills.md.
#
# The self-test runs here too, and it is the one `scripts/ci/*-test.sh` that
# belongs in a make target: it tests the very script this target runs, and it is
# pure bash with no toolchain, no cluster and no cloud credentials — unlike
# tf-helpers-test.sh (needs terraform) or kind-assertions-test.sh (needs a
# cluster), which is why those stay CI-only. Leaving it CI-only cost a full gate
# cycle on #694: `make skills-check` passed clean while the skills job failed on
# a piped `grep -q` the self-test exists to reject.
# One line with `&&`, not two recipe lines: `gates.md` renders a multi-line
# recipe as `a; b`, which reads as "b runs even if a fails" when make in fact
# stops at the first failing line. `&&` makes the fail-fast explicit in both
# places rather than only in make's semantics.
skills-check:
	bash scripts/ci/skills-check-test.sh && bash scripts/skills-check.sh all

# Tracked shell scripts live under scripts/ (CLAUDE.md conventions). CI runs
# the same script in rust.yml's `workflow pins (S9)` job, ungated.
scripts-home-check:
	bash scripts/ci/scripts-home-check.sh

# The other half of the same convention: shell that has no FILE, living in a
# workflow's `run:` block, which scripts-home-check.sh cannot see. Budgeted
# rather than banned — see the script's header. Its self-test runs here for
# the reason skills-check-test.sh does: it is pure bash with no toolchain, no
# cluster and no credentials, and it tests the very script this target runs.
# `&&`, not two recipe lines, so the fail-fast is explicit in gates.md too.
workflow-shell-check:
	bash scripts/ci/workflow-shell-check-test.sh && bash scripts/ci/workflow-shell-check.sh

# The classifier behind the `pull request contract` check (a ticket link and a
# post-merge ledger). It is in `make check` because the COMMITTED TEMPLATE is
# one of its fixtures: an edit to .github/pull_request_template.md that drops a
# ledger marker would otherwise surface only as a red check on every pull
# request opened afterwards, with nothing in the diff that mentions CI.
pr-contract-check:
	bash scripts/ci/pr-contract-test.sh

# #549's acceptance check, kept as a gate instead of a closed issue: exactly one
# `aws_iam_access_key` in the Terraform tree (`builder`), and no static
# ER_S3_* credential assignment. Self-test first, same reasoning as its two
# neighbours — a detector breaks by noticing nothing, which reads as a clean
# tree. `&&`, not two recipe lines, so the fail-fast is explicit in gates.md.
tf-static-credentials-check:
	bash scripts/ci/tf-static-credentials-check-test.sh && bash scripts/ci/tf-static-credentials-check.sh

# The fast PR-equivalent backend gate (mirrors rust.yml's lint + test tiers),
# plus the skill gate — a stale generated reference is the same class of defect
# as a stale generated entity artifact.
check: fmt lint doc entities-check skills-check scripts-home-check workflow-shell-check pr-contract-check tf-static-credentials-check test

# The gated e2e tiers: the DuckLake + extension pipeline and the Postgres
# control-plane queue/dispatch. Needs the DuckDB extensions and a local
# Postgres (see rust.yml's ducklake-e2e job for the service definition).
# Point ER_TEST_PG_DSN at your Postgres, e.g.
#   make check-e2e ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er'
check-e2e:
	cd rust && DEDUPE_E2E_DUCKLAKE=1 DEDUPE_E2E_EXTENSIONS=1 ER_TEST_PG_DSN="$(ER_TEST_PG_DSN)" \
	  cargo test -p dedupe-cli -p er-cli -p er-server -p er-entity-diff

# --- agent tooling: repo-local Symposium (part of the local dev install) ---
#
# Symposium (https://github.com/symposium-dev/symposium) matches plugins against
# the rust/ dependency graph and installs the Claude Code skills they ship —
# currently the error-handling and rust-best-practice sets pulled in by our
# thiserror/anyhow deps.
#
# Everything stays inside the repo instead of the developer's home directory:
# the cargo-agents binary in .tools/bin, config + cache in .symposium, and the
# skills in rust/.claude/skills. All three are gitignored except the config —
# the binary is a 15MB per-platform artifact, so it is fetched here rather than
# committed. scripts/symposium is the wrapper that sets SYMPOSIUM_HOME and runs
# from rust/ (symposium finds its project via `cargo metadata`).
#
# Re-run after changing rust/ dependencies; a SessionStart hook also re-syncs.
symposium:
	scripts/symposium install
	scripts/symposium sync

# --- frontend (standalone pnpm project; keeps its own gate) ---

# The PR-equivalent gate: lint + format + types + unit (with the coverage floor)
# + the BFF authz gate, then the mock-backed Playwright tier (visual baselines
# compare on Linux only; see frontend/README.md).
frontend:
	cd frontend && corepack pnpm install --frozen-lockfile && corepack pnpm check && corepack pnpm build && corepack pnpm test:e2e

# The supply-chain gate: HIGH/CRITICAL advisories + the license allowlist. The
# frontend's mirror of cargo-deny. Needs network, so it is its own target the way
# check-e2e is (see .github/workflows/frontend.yml, job `pnpm audit + licenses`).
frontend-audit:
	cd frontend && corepack pnpm audit:check && corepack pnpm licenses:check

# The browsable coverage report. The FLOOR is enforced by `pnpm check` on every
# PR (vitest.config.ts coverage.thresholds); this just writes frontend/coverage/.
frontend-coverage:
	cd frontend && corepack pnpm exec vitest run --coverage

# --- frontend dev substrate --------------------------------------------------
#
# Restored here after the all-Rust cutover (9a55504) dropped them: that commit's
# message says the frontend targets were "retained", but these five went with the
# Python ones, leaving frontend/README.md and frontend/dev/run.sh:97 calling
# commands that no longer existed. Their scripts were never deleted.
#
# `frontend-seed` is back (W0.5b), reimplemented as frontend/dev/seed.mjs after
# the Python frontend/dev/seed.py was deleted in 7cc444f. That deletion is what
# made the full-stack e2e tier dormant; it is no longer.

# The whole dev stack in one command (container + migrations + er-server + seed + web).
run:
	bash scripts/frontend-dev/run.sh

# The control-plane Postgres container + erweb migrations + er-server; blocks.
frontend-dev:
	bash scripts/frontend-dev/up.sh

# Seed the demo tenant: org, api keys, published config, base_10 resolved, logins.
# Needs `make frontend-dev` running in another terminal.
frontend-seed:
	@bash -c 'set -euo pipefail; \
	  export REPO_ROOT="$$PWD"; \
	  source scripts/frontend-dev/env.sh; \
	  node frontend/dev/seed.mjs'

# The full-stack tier: Playwright against the real stack. Needs `make frontend-dev`
# and `make frontend-seed`. Not on the PR path — this is the release gate.
frontend-e2e:
	cd frontend && FRONTEND_E2E=1 corepack pnpm test:e2e:full

# Regenerate visual baselines inside the pinned Playwright Linux image so they
# are stable across contributor machines.
frontend-baselines:
	bash scripts/frontend-dev/update-baselines.sh

frontend-dev-reset:
	docker compose -f frontend/dev/compose.yaml down -v
	rm -rf frontend/dev/.state

# --- container images (replaces the deleted ci.yaml image build) ---
#
# Build + push the Rust control-plane image to ECR by git sha. The image holds
# ONE binary serving all three entry points — er-server, er-dispatcher and the
# `er` pipeline CLI — dispatched on argv[0], with the latter two as symlinks;
# see rust/Dockerfile. Then `make dev-up TAG=<sha>` rolls it out.
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

# Apply the cluster-scoped objects: the Karpenter NodePools (app / runner-sm /
# runner-l) and the task PriorityClasses (batch-orchestration-design §7.1 —
# cluster-scoped, so no per-namespace chart can carry them).
#
# These are Kubernetes manifests, not AWS API objects, so Terraform does not own
# them (infra/terraform/modules/eks/main.tf says so) and no chart renders them.
# Until this target existed nothing applied them at all: they were applied by
# hand once, in plan-to-aws task 1.3, and an edit to a disruption setting
# changed nothing anywhere until someone remembered to re-apply it. `infra.yml`
# does not even trigger on these paths, so there is no drift gate either —
# `--dry-run=server` here at least shows what would move.
#
# infra/k8s/nodepools/measure/ is deliberately NOT applied: it holds the
# throwaway pool scripts/measure_runner_startup.sh creates and deletes.
cluster-objects:
	kubectl --context $(ER_DEV_CONTEXT) apply -f infra/k8s/nodepools/
	kubectl --context $(ER_DEV_CONTEXT) apply -f infra/k8s/priorityclasses/
	kubectl --context $(ER_DEV_CONTEXT) get nodepools priorityclasses

# The old name, kept callable because muscle memory and docs point at it.
nodepools: cluster-objects

# The kind probe (batch-orchestration-design §11 Phase 2): the executor end to
# end on a local cluster — real pods, real projected tokens against the real
# API server's JWKS, adoption across a dispatcher restart, cancel. kind-up
# builds the er-api image locally (long on first run; ER_KIND_SKIP_BUILD=1
# reuses it); check-kind drives the four probe phases.
kind-up:
	bash scripts/kind_env.sh up

kind-down:
	bash scripts/kind_env.sh down

check-kind:
	bash scripts/check_kind.sh

# Measure runner startup on the live cluster (.claude/skills/infra-platform/references/infrastructure.md §18.16).
# Run once per image to build a before/after record:
#   make measure-runner-startup IMAGE=<ecr>/er-api:<sha> ARM=before
measure-runner-startup:
	@test -n "$(IMAGE)" || { echo "usage: make measure-runner-startup IMAGE=<ref> ARM=before|after"; exit 2; }
	@test -n "$(ARM)" || { echo "ARM=before|after required"; exit 2; }
	bash scripts/measure_runner_startup.sh --image $(IMAGE) --arm $(ARM) \
	  $(if $(TRIALS),--trials $(TRIALS)) $(if $(ARMS),--arms $(ARMS)) $(if $(OUT),--out $(OUT))

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

# Back up in under a minute. kubectl, not `helm upgrade`, for speed: a
# `helm upgrade --wait` re-runs the release's hooks (including the schema
# migrate) to put back a replica count. Aurora resumes from 0 ACU on the first
# connection (~15s).
#
# This comment used to give a different reason — that helm's three-way merge
# "sees no manifest diff after a kubectl scale, so it would leave the replica
# counts at zero". That is Helm 2 behaviour and it is false here, which matters
# because the deploy's third phase retires a colour with exactly this `kubectl
# scale` and would be unsound if it held. Measured on kind against helm 3.17:
# install at 1, `kubectl scale` to 0, `helm upgrade` with the value unchanged,
# and the live object goes back to 1 — Helm 3 patches the live object against
# the rendered manifest, not against the previous one. Decision: D-BLUEGREEN.
#
# The dispatcher too, because `dev-suspend` scales `--all` to zero. Resume used
# to name only web-api — written when the dispatcher ran in-process inside that
# pod — so after the §6.7 split a suspend/resume cycle left `er-dispatcher` at
# zero replicas permanently, and the queue stopped draining with nothing in the
# web-api pod's events or logs to say why.
#
# But NOT `--all`, now that web-api is two Deployments: that would wake the
# inactive colour as well, so both colours would serve and the preview Service
# would be pointing at a live pod. The live colour is READ from the Service
# selector rather than assumed — the same single source of truth the cutover
# uses, so this cannot drift from whichever colour is actually serving.
dev-resume:
	@test -n "$(DEV)" || { echo "usage: make dev-resume DEV=<name>"; exit 2; }
	set -e; \
	colour=$$(kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) get svc er-web \
	  -o jsonpath='{.spec.selector.er\.dupezero\.com/color}'); \
	test -n "$$colour" || { echo "could not read the live colour from svc/er-web — is er-platform installed?"; exit 1; }; \
	echo "resuming the live colour: $$colour"; \
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) scale deployment web-api-$$colour er-dispatcher --replicas=1; \
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) rollout status deployment/web-api-$$colour --timeout=300s; \
	kubectl --context $(ER_DEV_CONTEXT) -n $(DEV) rollout status deployment/er-dispatcher --timeout=300s

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
	bash scripts/obs_up.sh

# Rebuildable local caches only.
clean:
	cd rust && cargo clean
