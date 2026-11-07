# Agentic workflows, and the dispatched agent

Moved out of the skill body: `ci-and-supply-chain/SKILL.md` is loaded at the
start of every session that touches CI, and this subsystem is **merged but
inert** — it has no secret to run on, whichever engine it declares. Detail about
a feature that cannot currently run belongs one hop away rather than in the
body, which is also what keeps the body under its 500-line authoring limit.

## Contents

- [Agentic workflows](#agentic-workflows)
- [The dispatched agent (`agent-task.yml`)](#the-dispatched-agent-agent-taskyml)

### Agentic workflows

`.github/workflows/*.md` are [`gh aw`](https://github.github.io/gh-aw/) agentic
workflows. **Edit the `.md`; `gh aw compile` emits the `.lock.yml` beside it.**
A `.lock.yml` is compiler output — treat it exactly like any other generated
file here and never hand-edit it. `references/required-checks.md` tabulates
which ones exist, what fires each, and what each is permitted to do.

**None of them may ever be a required check.** A model run is non-deterministic
and rate-limited, so requiring one would gate every merge on a retry. That is
also the one thing they have that `rust.yml` and `frontend.yml` do not: because
they are not required, they *are* allowed `paths:` and `if:` filters. The rule
in CLAUDE.md is about required workflows specifically.

They also hold almost all of the repo's GitHub write scopes.
`pull-requests: write` and `issues: write` appear nowhere else, and
`contents: write` appears in one other place: the `agent` job of
`agent-task.yml` (below). The other hand-written workflows are `contents: read`
plus `id-token: write` for OIDC.

**The model's own job holds none of them.** gh-aw compiles each workflow into
five or six jobs, and the `agent` job that runs the model gets only read scopes;
`contents: write` and friends live in the separate `safe_outputs` and
`conclusion` jobs that run after it exits. gh-aw's **safe outputs** are declared,
capped operations (`add-comment`, `push-to-pull-request-branch`, …) which those
jobs validate — so the model's output is data they check rather than commands
they obey. Worth verifying rather than trusting, and easy to: extract the
per-job `permissions:` blocks from a `.lock.yml`.

One exception, stated because "the agent never writes" is otherwise an
over-claim: the two agents that compile Rust carry `id-token: write` *on the
agent job*, because `shared/rust-gate.md` exchanges it for the sccache role.
That mints an AWS credential, not a GitHub one, and the role is
`er-ci-sccache-read` — `s3:GetObject` and `s3:ListBucket` on one bucket, with no
`DeleteObject` in the policy at all. Read access to compiler output is the whole
blast radius, and it is read, so an agent cannot plant object code a later
`main` build consumes.

**The zero-repository-secrets invariant does not survive them, and the claim
that it did was false.** This section used to say `engine: copilot` draws on
`copilot-requests: write` against the job token, so no secret was needed. On
first real execution (2026-11-03, the run after #359 landed) all four agents
failed at `activation`:

```
Error: None of the following secrets are set: COPILOT_GITHUB_TOKEN
COPILOT_GITHUB_TOKEN is required by GitHub Copilot CLI.
```

The generated `activation` job runs `validate_multi_secret.sh
COPILOT_GITHUB_TOKEN` and hard-fails without it. **There is no zero-secret path
to running these at all** — `copilot` needs `COPILOT_GITHUB_TOKEN`, `claude`
needs `ANTHROPIC_API_KEY` (an API key; gh-aw does not accept a Claude
subscription OAuth token), and every other engine wants its own key. The
invariant and the feature are mutually exclusive, which is the thing to know
before reasoning about either.

They now declare `engine: claude` and need `ANTHROPIC_API_KEY` — **which is not
set**, so they still fail at `activation`, on a different secret name. Nothing
about leaving Copilot made them runnable. The model is chosen per workflow by
whether the agent exercises judgement:

| Workflow | Model | Why |
|---|---|---|
| `dependency-upgrade-pilot` | `claude-sonnet-5` | verdicts a version range |
| `ci-triage` | `claude-sonnet-5` | classifies a red run |
| `generated-artifact-healer` | `claude-haiku-4-5` | re-runs a known generator |
| `skill-freshness-auditor` | `claude-haiku-4-5` | greps prose against code |

Haiku is the cheapest current model and fine for mechanical work; it is the
wrong default for the two judgement agents, because a confidently wrong verdict
on a dependency bump is the failure #376 already cost three red PRs for. **Never
set `effort` on a Haiku workflow** — Haiku 4.5 rejects the parameter.

One consequence of leaving Copilot: `GH_AW_MAX_DAILY_AI_CREDITS` (5000) is
denominated in Copilot credits and does not bind a BYO-key run, so the spend
ceiling is whatever limit is configured Anthropic-side. These trigger on every
dependabot push and every red CI run, so the moment a key is set that limit is
load-bearing rather than belt-and-braces — set it before the secret, not after.

They fail **closed**, which is the one thing that went right: `activation`
failing leaves `agent`, `detection` and `safe_outputs` all `skipped`, so a
missing or revoked key costs red runs, never an unintended write.

Two interactions with the gates above worth knowing. `rust.yml`'s `detect`
counts these paths as agent-only, so a prompt edit skips the backend gate —
which is safe precisely because the always-on `skills` job is not gated on
`detect` and still catches generated-reference drift. And the SHA-pin rule
applies to the generated lock files too; `gh aw compile` pins them for you.

### The dispatched agent (`agent-task.yml`)

`agent-task.yml` is the fire-and-forget agent: `workflow_dispatch` only, one
task per run, ending in a pull request. It is a plain workflow running
`anthropics/claude-code-action`, not gh-aw, because it authenticates with a
Claude subscription token. It copies gh-aw's split instead. The `agent` job
holds only the workflow token, with `contents: write`, and checkpoints to
`agent-wip/<task>`. A separate `publish` job, on a fresh runner that never
checks out the agent's tree, mints a GitHub App token and opens the PR. Design
and rationale: `references/agent-fleet.md`.

It expects two secrets beyond the gh-aw agents' `ANTHROPIC_API_KEY`, and unlike
that one they are to be **environment-scoped**: `CLAUDE_CODE_OAUTH_TOKEN` in an
`agent` environment and `AGENT_APP_KEY` in `agent-publish`, both restricted to
`main`, so a PR branch that edits a workflow cannot read them. **Neither
environment exists yet** — the repository's only environments are the three
`infra-apply-*` ones — so the paragraph above is the rule for creating them, not
a description of what is configured, and the isolation it describes is not in
force today. When they are created: do not put either secret at repository
scope, and do not let the `agent` job reference `AGENT_APP_KEY`.

**Its `agent` job must not gain `id-token: write`.** `er-ci-tf-plan` trusts any
ref of this repository, so a job that can mint an OIDC token can read Terraform
state. That is why it restores `rust-cache` but not the S3 sccache, and why its
`CARGO*`/`RUST*` env must stay identical to `rust.yml`'s `lint` job.
