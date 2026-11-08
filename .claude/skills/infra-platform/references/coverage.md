# What CI covers, and what actually gets applied

Three inventories this skill used to state by hand, now derived. Each is a
**set difference**: the point is not the list, it is the gap the list reveals,
because a stack nobody plans and a manifest nobody applies both fail silently.

## Contents

- [Why this file is generated](#why-this-file-is-generated)
- [How to read it](#how-to-read-it)
- [Terraform stacks and the tiers that cover them](#terraform-stacks-and-the-tiers-that-cover-them)
- [Where each `infra/k8s` path is applied from](#where-each-infrak8s-path-is-applied-from)
- [NodePool manifests the cluster apply reaches](#nodepool-manifests-the-cluster-apply-reaches)

## Why this file is generated

`infra-platform` twice stated an inventory in prose and twice got it wrong: the
NodePool count said three while four `karpenter.sh/v1` manifests sat in the
tree, and the non-chart manifest table omitted `priorityclasses/` after it was
added. Both are the defect `.claude/skills/skill-writer/references/agent-skills.md` forbids — a hand-written
count in this repo goes stale within weeks.

Every row below is read out of the tree by `scripts/skills-check.sh regen`, so
it cannot disagree with what is there. Do not hand-edit inside the markers.

## How to read it

The three tiers in `infra.yml` are **not** one list. `validate` enumerates
stacks in a shell `for` loop; `plan` and `apply` each enumerate them in a
`matrix.include`. Nothing cross-checks the three, so a stack can sit in one and
be missing from another with no job turning red — that is the gap the first
table reports.

The second table answers a question the old hand-written one did not: *what
applies this path?* A path referenced by nothing in the tree is applied by hand
or not at all. That is not necessarily a defect — but it is never what a reader
assumes, and it is the state two paths are in.

The third table exists because `kubectl apply -f <dir>` is **non-recursive**.
That single fact is why a measurement-only pool can live under
`infra/k8s/nodepools/measure/` and stay out of the cluster without any
exclusion list to maintain. A pool added in a *new* subdirectory inherits the
same silence, which is what this table surfaces.

<!-- BEGIN GENERATED: infra-coverage -->
### Terraform stacks and the tiers that cover them

| Stack | `validate` | `plan` | `apply` |
|---|---|---|---|
| `bootstrap/organization` | yes | — | — |
| `bootstrap/state-backend` | yes | yes | — |
| `envs/core` | yes | yes | yes |
| `envs/dev` | yes | yes | yes |
| `envs/prod` | yes | yes | yes |

Every stack in the tree is reached by at least one tier.

### Where each `infra/k8s` path is applied from

| Path | Referenced by |
|---|---|
| `charts` | `Makefile`, `scripts/check_kind.sh`, `scripts/ci/bluegreen-acceptance.sh`, `scripts/ci/charts-check.py`, `scripts/ci/charts-render-validate.sh`, `scripts/ci/deploy-cutover.sh`, `scripts/ci/deploy-inactive-colour.sh`, `scripts/ci/deploy-namespace-chart.sh`, `scripts/ci/helm-lint.sh`, `scripts/ci/infra-detect.sh`, `scripts/kind_env.sh` |
| `kind` | `scripts/kind_env.sh` |
| `network-policy-controller.yaml` | `.github/workflows/cluster-objects.yml`, `scripts/cluster_objects.sh` |
| `nodepools` | `.github/workflows/cluster-objects.yml`, `scripts/cluster_objects.sh`, `scripts/measure_runner_startup.sh` |
| `observability` | `Makefile`, `scripts/obs_up.sh`, `scripts/otel-pii-allowlist-probe.sh` |
| `priorityclasses` | `.github/workflows/cluster-objects.yml`, `scripts/cluster_objects.sh`, `scripts/kind_env.sh` |
| `secrets` | `.github/workflows/cluster-objects.yml`, `scripts/cluster_objects.sh` |
| `storage` | `Makefile`, `scripts/ci/deploy-namespace-chart.sh` |

### NodePool manifests the cluster apply reaches

`kubectl apply -f <dir>` is non-recursive, and `cluster-objects`
names two directories: the top level, applied to whichever cluster is
targeted, and `<env>/` for the pools that differ per environment.
Any other subdirectory is reached by nothing.

| Manifest | Applied |
|---|---|
| `dev/app.yaml` | yes — `dev` only |
| `measure/runner-probe.yaml` | no — subdirectory no environment is named after |
| `prod/app.yaml` | yes — `prod` only |
| `runner-l.yaml` | yes — every cluster |
| `runner-sm.yaml` | yes — every cluster |
<!-- END GENERATED: infra-coverage -->
