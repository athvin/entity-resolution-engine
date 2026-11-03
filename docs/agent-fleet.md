# Agent fleet: dispatched coding agents that end in a pull request

*Status: Phase 1 implemented (`.github/workflows/agent-task.yml`,
`scripts/agent.sh`), not yet exercised — it needs the one-time setup in §2.
Phases 2–5 are design only.*

The goal: run one command from a laptop and get an autonomous Claude Code run on
remote compute. The run has the repository, solves the task without
intervention, and ends in a pull request. All review happens in GitHub, under the
same required checks as a human's PR.

```
laptop ── agent "fix X" ──► gh workflow run agent-task.yml (on main)
                                   │
              ┌────────────────────┴─────────────────────┐
              │ job: agent          environment: agent   │  GITHUB_TOKEN: contents:write
              │  checkout → agent-wip/<task>             │  no App key, no id-token
              │  claude-code-action (agent mode)         │
              │   commits + pushes every step ──────────►│── agent-wip/<task>  (no PR, no CI)
              │  structured result {status,title,body}   │
              └────────────────────┬─────────────────────┘
                                   │ needs (always)
              ┌────────────────────┴─────────────────────┐
              │ job: publish    environment: agent-publish│ fresh runner, no checkout
              │  mint App token (now, not an hour ago)   │
              │  agent/<task> := agent-wip/<task> (ff)   │── agent/<task> ──► PR (as the App)
              │  open PR, or update it on resume         │                     │
              └──────────────────────────────────────────┘          rust/frontend/infra CI
                                                                     human review + merge
```

## 1. What the workflow does, and why it is two jobs

**`agent`** runs `anthropics/claude-code-action` in agent mode: a `prompt` input
makes the action run it directly, with no `@claude` trigger. This was checked
against the action's source at the pinned version, not taken from its docs. The
job's GitHub identity is the workflow token, with `contents: write` and
`pull-requests: read`. The action swaps the checkout credential for whatever
`github_token` it is given, so Claude's `git push` uses that token. The prompt
tells Claude to commit and push after every meaningful step. A checkpoint step
with `if: always()` pushes whatever is left on failure, turn exhaustion or
timeout. Claude finishes by returning a `--json-schema` structured result:
`status` (`complete | partial | blocked`), `title` and `body`.

**`publish`** runs on a fresh GitHub-hosted runner. It never checks out the
agent's tree. It mints the App token, fast-forwards `agent/<task>` to
`agent-wip/<task>`, and opens the PR as the App. The PR is ready for review only
when the agent step succeeded and reported `complete`; otherwise it is a draft.
On a resume it updates the existing PR instead.

The split is the design. Each property below follows from the structure, not
from what the prompt asks for:

| Property | Mechanism |
|---|---|
| The model never holds the App private key or an App token | The key is a secret in the `agent-publish` environment, which only `publish` references. `publish` runs on a different machine after the model has exited |
| Agents cannot change their own powers | GitHub refuses any push that touches `.github/workflows/**` from a token without the `workflows` permission, and the workflow token cannot be granted it. Branch protection keeps every token off `main` |
| A checkpoint costs no CI | Checkpoints go to `agent-wip/<task>`, which never has a PR. Events from the workflow token start no workflows anyway |
| CI runs on the PR like a human's | The PR is opened, and `agent/<task>` moved, by the App, and App-caused events do start workflows. Exactly one CI run per publish |
| No token expiry mid-task | App installation tokens live one hour. `publish` mints its own at the end, so a 90-minute agent run cannot outlive its credential |
| A spot loss costs minutes, not the task | Pushes every step, plus `agent-resume <task>`, which checks out `agent-wip/<task>` and tells the agent to read its own history and PR first |
| Resumes cannot race | `concurrency` is keyed on the task id, so two runs of one task queue rather than fight over the branch |

**The agent job has no `id-token: write`, on purpose.** `er-ci-tf-plan` trusts
`repo:…:*` (`infra/terraform/envs/core/ci-terraform.tf`). Any job in this
repository that can mint an OIDC token can therefore assume it and read
Terraform state, which holds sensitive values in cleartext. An LLM with
unrestricted Bash and network should not be one of those jobs. The cost is the
S3 sccache. A warm dependency tree still comes from restoring the `rust-cache`
entry that `rust.yml`'s `lint` job writes on `main`. That only hits because the
job's `CARGO*`/`RUST*` environment matches `lint` exactly (rust-cache keys on
it). Phase 3 gets the S3 cache back through Pod Identity instead (§5).

**Verification inside the run is the cheap half of the gate.** The agent runs
what `.github/workflows/shared/rust-gate.md` defines for the gh-aw agents: fmt,
clippy `-D warnings`, cargo-deny, `er-entity verify`, and skills-check. It does
not run `make check`, `cargo test --workspace` or the e2e suite. The PR's CI
runs those, and a hosted runner has neither the disk nor the minutes. On ARC
with local NVMe (§5) this can widen to the full `make check`.

### Inputs

| Input | Type | Notes |
|---|---|---|
| `prompt` | string | The task. Reaches the action as an env var and a file, never a shell |
| `resume` | string | A task id (the run id that started it). Validated as digits before it touches a ref |
| `model` | choice | `default`, `opus`, `sonnet` or `haiku`. A choice rather than free text, because it is spliced into `claude_args` |
| `max_turns` | number | Default 80. A number for the same reason |

### Laptop side

`scripts/agent.sh` is sourced from a shell rc file and provides `agent`,
`agent-file`, `agent-resume`, `agents`, `agent-prs` and `agent-watch`. Every
dispatch targets `main`, the only ref both environments accept.

## 2. One-time setup (Phase 0)

None of this is in the tree; it is repository and account configuration.

1. **A GitHub App for the publish identity**, installed on this repository only.
   Repository permissions: *Contents* read/write, *Pull requests* read/write,
   *Metadata* read. **Not** *Workflows*: nothing in this design needs it, and
   its absence is half of "agents cannot change their own powers". *Issues*
   only at Phase 5.
2. **Environment `agent-publish`**, deployment branches restricted to `main`.
   It needs the variable `AGENT_APP_CLIENT_ID`, the App's *client ID*, because
   `create-github-app-token` v3 deprecates `app-id`. It also needs the secret
   `AGENT_APP_KEY`, the App's private key.
3. **Environment `agent`**, deployment branches restricted to `main`. It needs
   the secret `CLAUDE_CODE_OAUTH_TOKEN`, from `claude setup-token` on a laptop.
   Note the token's expiry, and set a reminder to rotate it before then.
4. **The ruleset on `main`**, if it does not already require it: a pull request
   with one approving review, on top of the required checks it already has. The
   App is not a bypass actor.
5. **Settings → General → Automatically delete head branches**, so a merged
   `agent/<task>` goes away. `agent-wip/*` is not a PR head and is not deleted.
   See §7.

**This adds two stored secrets.** #439 already gave up the zero-secret
invariant for the gh-aw agents (`ANTHROPIC_API_KEY`, repository scope). These two
are environment-scoped to `main` instead, which is what keeps them from a PR
branch that edits a workflow. The trade is deliberate, and three alternatives
exist:

- **A dedicated Anthropic API key** in the `agent` environment, passed as
  `anthropic_api_key` instead of the OAuth token. That means API billing with an
  Anthropic-side spend limit, rather than the subscription's shared usage cap.
  Mint it separately, for the reason #439 gives: never the product's key, which
  touches customer data.

- **Anthropic workload identity federation.** The pinned action accepts
  `anthropic_federation_rule_id` + `anthropic_organization_id` and exchanges the
  job's GitHub OIDC token for API access. That means no stored Claude secret, at
  API rather than subscription billing. It needs `id-token: write` on the agent
  job, so it should wait until `er-ci-tf-plan`'s trust is narrowed (§7).
- **Bedrock** (`use_bedrock: true`). The permission half exists in
  `envs/dev/bedrock.tf`, but model access is still blocked on the account
  owner's use-case form, so it is not usable today.

The Max subscription's limits are shared by every parallel agent. Whether
Anthropic's consumer terms cover this volume of automated use is for the account
owner to confirm, not something this document can settle.

## 3. Where this departs from the plan it came from

The originating plan was written without this repository's conventions in view.
Each row is a deliberate correction:

| The plan said | Here | Why |
|---|---|---|
| Agent job holds the App token, pushes the PR branch and runs `gh pr create` itself | Two jobs, `agent-wip/*` checkpoints, App only in `publish` | §1: key never on the model's machine, no 1-hour expiry, one CI run per publish, workflow edits impossible |
| `id-token: write` on the agent job | None | `er-ci-tf-plan` is assumable from any ref of this repo |
| `concurrency: agent-${{ github.run_id }}` | Keyed on the task id | A per-run group never contends. Per-task serialises resumes |
| `model`/`max_turns` as free strings interpolated into `claude_args` | `choice` / `number` | `claude_args` is shell-parsed. A free string there is a flag-injection point |
| Actions by tag (`@v1`, `@v2`, `@v4`) | Commit-SHA pins with a `# vTAG` comment, each release at least two weeks old | Repository policy (ci-and-supply-chain skill) |
| `create-github-app-token` with `app-id` | `client-id` (v3.3.0) | `app-id` is deprecated in v3 |
| A new `CLAUDE.md` with `cargo test` / `cargo clippy` | The existing `CLAUDE.md`, plus the rust-gate cheap half | The gate here is `make check`, cargo runs from `rust/`, and agents defer the long half to CI |
| Karpenter NodePool **plus** an SQS interruption queue and EventBridge rules | A `karpenter.sh/v1` NodePool only | EKS Auto Mode runs Karpenter and owns interruption handling. Installing or configuring Karpenter here is wrong (infra-platform skill) |
| IRSA for the runner service account | EKS Pod Identity | There is no OIDC provider for workloads. IRSA annotations do nothing here |
| `infra/modules/arc`, `infra/envs/prod/agents.tf`, `helm_release` | Values under `infra/k8s/arc/`, installed by a script on the `scripts/obs_up.sh` pattern, on **`er-dev`** | Terraform owns the cloud and Helm owns the cluster. Prod never runs agents |
| ARC `containerMode: kubernetes` + PVC + `container: image` | The agent image **is** the runner image | No container hooks, no RWO volume to schedule around, workspace on local NVMe, and Pod Identity reaches the process directly |
| `devenv:latest` | `er-agent:<commit sha>` | ECR repos here are `IMMUTABLE`, promotion is by digest, and there is no `latest` |
| A separate `devenv-image.yml` with a weekly rebuild | A detect-gated job in `images.yml`, base bumped by Dependabot | One image workflow and one push role. A weekly rebuild of the same SHA collides with immutable tags |
| `maxRunners: 10` as the cost cap | Blocked: the account vCPU quota is **8** | §5. No runner-shaped node can be placed today |

## 4. Phase 2: one image for the laptop and the agent

- **One Dockerfile**, `.devcontainer/Dockerfile`, `FROM
  ghcr.io/actions/actions-runner@sha256:<digest>`. It bakes in the toolchain the
  workflow installs per run today: rustup plus the `rust/rust-toolchain.toml`
  toolchain, rustfmt, clippy, mold, sccache, cargo-deny, Node with corepack and
  pnpm, Terraform, `gh`, and the DuckDB CLI. `.devcontainer/devcontainer.json`
  builds from it, so the laptop and the agent share one definition. Devcontainer
  lifecycle hooks (`postCreateCommand`, features) do not run on a runner pod, so
  anything there must move into the Dockerfile.
- **ECR**: add `er-agent` to `local.images` in `envs/core/registry.tf`
  (immutable, scan-on-push, last 30), and its ARN to `er-ci-ecr-push`'s
  repository list.
- **Build**: a job in `images.yml` gated by its `detect` on `.devcontainer/**`,
  tagged by commit SHA. Add `.devcontainer/**` to the workflow's `push.paths` and
  the Dockerfile to its `pull_request.paths`. The `verify` job then builds it on
  the PR that changes it.
- **Freshness**: a Dependabot `docker` entry for `.devcontainer/` bumps the base
  digest, so every rebuild is a reviewed commit with its own tag.

## 5. Phase 3: Actions Runner Controller on `er-dev`

**Prerequisite, blocking: the vCPU quota.** *All Standard Spot Instance
Requests* (`L-34B43A08`) and *Running On-Demand Standard instances*
(`L-1216C47A`) both read 8 vCPU applied. The 2026-10-11 increase requests closed
without being granted. The `app` and `system` nodes take 4, which leaves less
than one 8-vCPU node. An ARC runner pod would sit `Pending` with no scheduling
event, forever (aws-account-operations skill, `docs/infrastructure.md` §5). The
quota is shared with prod, since there is one account. Size the request as
`maxRunners × node vCPU` plus the pipeline runner pools plus 4. At 8-vCPU nodes,
a cap of 5 agents needs about 48 for agents alone. Ask for 64 and confirm the
*applied* value afterwards.

Then:

- **A NodePool `agents`** (`infra/k8s/nodepools/agents.yaml`). It is a
  `karpenter.sh/v1` NodePool on the Auto Mode `default` NodeClass. Spot with an
  on-demand fallback, `c6id`-family `2xlarge` (8 vCPU, local NVMe that Auto Mode
  stripes RAID0), tainted `er.dupezero.com/agent=true:NoSchedule`.
  `consolidationPolicy: WhenEmpty` with a **short** `consolidateAfter`: this is
  exactly the "fleet of CI runners" case `runner-sm.yaml`'s comment says should
  be short. `expireAfter: 72h`, and `karpenter.sh/do-not-disrupt: "true"` on the
  runner pod. It is **its own pool, not `runner-sm`**. Those nodes run tenant
  pipeline Jobs, and a pod executing model-chosen code with a repository write
  token must not share a node with tenant data.
- **ARC**: the `gha-runner-scale-set-controller` and one `gha-runner-scale-set`
  named `arc-agents`, in namespace `arc-agents`. Chart versions are pinned in an
  `scripts/arc_up.sh` modelled on `obs_up.sh`, called by `make` and by
  `images.yml`'s `deploy-dev`. Values go in `infra/k8s/arc/`. Use `minRunners:
  0` and a `maxRunners` the quota can actually honour. The runner container
  image is `er-agent:<sha>`. Mount an `emptyDir` with an explicit `sizeLimit` at
  `/home/runner/_work`, and set `requests.ephemeral-storage`. These are the same
  two load-bearing limits `docs/infrastructure.md` §5.3 sets on pipeline runners.
- **ARC's own GitHub App**, separate from the publish App, with *Administration*
  read/write on this repository. Its key lives in Secrets Manager
  (`envs/dev/secrets.tf`), projected by External Secrets through the existing
  `ClusterSecretStore`.
- **Pod Identity**, never IRSA: a role trusting `pods.eks.amazonaws.com` with
  `s3:GetObject`/`ListBucket` on the sccache bucket only, and an
  `aws_eks_pod_identity_association` for `(er-dev, arc-agents, <runner SA>)`.
  That restores the S3 compile cache without giving the job an OIDC token.
- **Namespace guards**: a ResourceQuota (CPU, memory, ephemeral storage) as a
  second cost cap, and a default-deny ingress NetworkPolicy. Egress is the open
  item in §7.
- **Cut-over**: `runs-on: arc-agents`. Drop the toolchain steps the image now
  carries, and widen the prompt's verification to `make check`.

## 6. Phase 4: hardening

Already covered by Phase 1: per-task concurrency, resume, a checkpoint on
failure, a job summary, `--max-turns`, step and job timeouts, and a draft PR when
the agent did not finish. Remaining:

- **CI cost.** Every published agent PR pays the full Rust gate (14–42 min, see
  ci-and-supply-chain's build-cost reference). Phase 3's NVMe nodes are the
  lever if the required jobs ever move to ARC. That is a separate decision with
  its own required-check renaming hazards.
- **Generated-artifact drift.** `generated-artifact-healer` and `ci-triage` are
  capped to `main`, `bm/**` and `dependabot/**`. Adding `agent/**` would let the
  healer fix the most common agent failure, a stale generated file, on the
  agent's own PR.
- **Spot.** Auto Mode drains on the two-minute notice. A killed runner fails the
  job, and nothing re-queues it. `agent-resume` is the recovery. An on-demand-only
  scale set for long tasks is cheap to add once ARC exists.

## 7. Open items

1. **Narrow `er-ci-tf-plan`'s trust** from `repo:…:*` to the plan tier's
   `job_workflow_ref`. That is broader than this feature. It is the precondition
   for giving any agent job an OIDC token, including the federation auth option
   in §2.
2. **`agent-wip/*` cleanup.** These refs accumulate. Options: a scheduled job
   that deletes `agent-wip/<id>` once `agent/<id>`'s PR is closed, or a ruleset
   with a branch-name pattern lifecycle.
3. **Egress.** Plain NetworkPolicy is IP-based and cannot express "GitHub,
   Anthropic, crates.io, npm, ECR". Decide between an egress proxy and accepting
   open egress for a namespace that holds no AWS credential beyond sccache read.
4. **First runs.** The workflow has never executed. Dispatch a trivial task
   first, for example a docs typo. Confirm the checkpoint push, that `publish`
   opens the PR as the App, and that `rust.yml` starts on it without an approval
   click.
5. **The self-hosted runner platform fee** was announced and postponed. Re-check
   before Phase 3 changes the cost case.
