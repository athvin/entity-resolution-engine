# Working in this repo

## Product purpose

We build a configurable deduplication API. A customer should be able to define a
dataset and its source mappings, configure matching and survivorship, submit
records, run resolution, and retrieve results through a documented API. OpenAPI
is the customer integration contract for that workflow. Salesforce is our first
commercial integration; its appeal as an initial revenue channel is a hypothesis
to validate with customers. It does not define the core data model or the release
boundary for the API product. File, object-storage and SFTP integrations reuse the
same dataset, configuration, ingestion and result contracts.

Prioritize a complete customer API workflow before connector breadth. Separate
shared capabilities from provider-specific actions such as Salesforce writeback
and AppExchange distribution. For roadmap decisions, use `project-board` and its
[product direction](.claude/skills/project-board/references/product-direction.md);
read the live board for ordering and delivery status.

## The gate is `make check`, not raw cargo

Before calling backend work done, run **`make check`** from the repo root. It is
the PR-equivalent gate and mirrors `.github/workflows/rust.yml`:

```
make check   # = fmt + lint + doc + entities-check + skills-check
             #   + agent-bridge-check + scripts-home-check
             #   + workflow-shell-check + pr-contract-check
             #   + tf-static-credentials-check + test
```

`cargo fmt` / `cargo clippy` / `cargo test` on their own are **not** sufficient —
they skip `entities-check`, which is the entity-framework gate (`er-entity
verify`: regenerate every artifact from `entities/*/entity.yaml` and fail if a
committed file is stale, then plan every statement and run the E0xx passes).
A change that passes clippy and tests can still break that gate.

Some installed third-party skills (notably `rust-best-practice`, which symposium
syncs into `rust/.claude/skills/`) prescribe a generic finish line of `cargo fmt`
+ `cargo clippy --all --workspace` + `cargo test --all --workspace`. In this repo
that checklist is incomplete; `make check` supersedes it.

Other gates, each run on its own because they need external services:

| Command | What it covers | Needs |
|---|---|---|
| `make check` | the backend PR gate | — |
| `make check-e2e` | DuckLake pipeline + Postgres control plane | DuckDB extensions, a local Postgres via `ER_TEST_PG_DSN` |
| `make frontend` | the frontend PR gate: lint + format + types + authz + unit/component with the coverage floor, then the mock-backed Playwright tier | corepack pnpm |
| `make frontend-audit` | the frontend supply chain: HIGH/CRITICAL advisories + the license allowlist | network |
| `make entities-check` | just the entity-framework gate | — |
| `make skills-check` | just the agent-skill gate (authoring rules + generated references) | — |
| `make agent-bridge-check` | just the Codex config bridge (`.agents/skills`, `AGENTS.md`, the `.mcp.json` derivation) | — |

**The frontend gate blocks merges now.** `lint + types + unit`, `playwright
functional tier`, `pnpm audit + licenses` and `postgres.js driver contract` are
required status checks on `main` alongside the Rust and Terraform ones — before
2026-10-20 the frontend CI ran and was then ignored at merge time.
`.github/workflows/frontend.yml` therefore has no `paths:` filter: it uses the
same `detect`-job pattern `rust.yml` does, because a workflow a path filter
skips never reports, and a required check that never reports blocks every
backend-only PR forever. The required set itself is branch-protection
configuration rather than a file in the tree, so no count of it anywhere here
can be gated; read it from the repository settings.

`.claude/skills/frontend-testing/references/frontend-testing.md` is the frontend standard — which test tier a change
belongs in, what each gate is for, and the rule that every gate must be observed
*failing* before it is trusted.

**Those greens can be stale, and the merge bus is what re-earns them.** `main`'s
protection has `strict: false`, so a PR merges on a green it won against a base
that may since have moved — four PRs each green against an older `main` can all
merge and leave `main` red. `merge-bus.yml` batches the PRs **labelled
`merge-bus`**, proves the *combination* green through an ordinary PR, and only
then squash-merges the members. GitHub's own merge queue is this feature and
cannot be enabled here, because it is offered only for organization-owned
repositories and this one is user-owned — visibility is not the blocker — which
is why it is a workflow here. The
`ci-and-supply-chain` skill carries the rules; the one that bites is that
`merge bus` must never become a required check.

**A pull request is not done at green — it is done when `main` is green with the
change on it and its post-merge ledger is closed.** `/land-pr <n>` is the
procedure for getting there: the polling that does not accept a false green, the
merge rules (there is no auto-merge here, and a single-commit PR squashes to the
*commit* subject rather than the PR title), the post-merge watch over the
workflows no pull request can run, and the ledger that records what that watch
found. Use it for any PR being driven to merge rather than improvising the loop —
`gh pr checks` alone will report a PR green that compiled nothing.

**Every pull request closes a board ticket and carries a post-merge ledger.**
`.github/pull_request_template.md` is brief in every section except that ledger:
a checkbox block, inside stable `<!-- BEGIN POST-MERGE -->` markers, whose boxes
are ticked **after** the merge — in the merged body, with the evidence inline.
Three annotations, and the asymmetry is load-bearing: `n/a — <why>` and
`deferred — #<ticket>` tick the box, `blocked — <why>` leaves it empty, so **an
empty box means exactly one thing** — the follow-up did not happen. A merged
PR's body stays writable, which is what lets the record outlive the session that
did the work; the descriptive material (plan, discoveries, measurements) goes in
the template's collapsed Context block so the top of a body stays readable.
`pull request contract` (`pr-contract.yml`) enforces both clauses and reads
GitHub's own parsed closing references rather than the body text, so a
`Closes #<n>` that would not actually close anything does not pass.
`merge-bus/*` branches, Dependabot, reverts and the `no-ticket` label are
exempt — the first of those load-bearing, since the merge bus reads its own
PR's check rollup. The check is a **required context** as of 2026-11-07, added
only after it had been seen failing and after no open PR would be blocked by
it; the `ci-and-supply-chain` skill carries that order, which is the reusable
part. `scripts/ci/pr-ledger-audit.sh` sweeps merged PRs for ledgers an
interrupted session left open, and for deferrals that name no ticket — the one
annotation with a syntactic requirement, because otherwise it would be the way
to retire an item into nothing.

## Work in a worktree, cut from a fresh `main`

**Default to a git worktree for any change that will become a pull request.**
Several sessions share this clone, and the hazard is concrete: staging with
`git add -A` from one session commits another's half-finished work, and two
sessions on one branch interleave commits. A worktree removes both by giving the
session its own working directory.

Start of session, from the primary checkout, in this order — **the prune comes
first, because the disk is spent at creation and a new worktree is the thing
that runs out of it.**

**1. Delete the worktrees that are no longer needed.**

```sh
scripts/worktree-audit.sh         # verdict per worktree, with the reason
scripts/worktree-audit.sh --size  # same, plus du (slow)
```

It is **read-only**: it prints `KEEP` with the reasons, or `REMOVABLE` with the
`git worktree remove` line to run, and never removes anything itself. Run those
lines, then `git worktree prune` to drop admin entries for directories already
gone.

Four guards decide "no longer needed", and the one people reach for first is the
weakest:

- **`locked` is not how to tell, and neither is `lsof`.** Checking
  `lsof -a -p <claude pid> -d cwd` per `claude` process is proof of attachment
  when it hits, and proves nothing when it misses: the dominant pattern here is
  a session whose cwd is the **primary checkout** driving its worktree with
  `git -C <path>`, which holds no descriptor there. Measured with 19 worktrees
  open — 15 had a commit land in the previous 22 minutes, while only 7 were any
  process's cwd. One of them committed 12 seconds before being evaluated as
  unattached.
- **Local git activity is the signal that catches those.** The per-worktree git
  directory's `index` and `logs/HEAD` are rewritten by any operation a session
  performs in that tree, so their mtime is liveness. Do not substitute the age
  of the commit `HEAD` points at: a worktree cut from a week-old commit is brand
  new, and one cut from a commit that landed a minute ago may be abandoned. And
  read the mtime with `git --no-optional-locks status` — a plain `git status`
  refreshes the index and so overwrites the timestamp you came to read.
- **Tie the worktree to its pull request and read that.** A closed PR — merged
  or abandoned — means the worktree is finished. Ask by the worktree's HEAD
  commit rather than by branch, because harness-cut worktrees are detached and
  have no branch name to look up:

  ```sh
  sha=$(git -C <worktree> rev-parse HEAD)
  gh api "repos/{owner}/{repo}/commits/$sha/pulls" \
    --jq '.[]|"#\(.number) state=\(.state) merged=\(.merged_at != null)"'
  ```

  Do **not** substitute `git branch --merged`: squash merges leave the branch
  unreachable from `main`, and it once reported 2 of 69 branches as merged. A
  `422` from that endpoint means the commit is not on the remote at all — that
  is unlanded work, so the worktree stays.
- **A dirty tree stays.** `git worktree remove` refusing is **information, not
  an obstacle** — uncommitted work there has not landed anywhere. Do not reach
  for `--force` to get past it; find out what the change was first.

**"Nothing is removable" is a normal answer** while the fleet is busy, and it is
what the audit reported with 19 worktrees open. Re-run when things are quiet
rather than forcing.

**2. Refresh `main`.**

```sh
git -C <primary checkout> switch main && git pull --ff-only origin main
```

**3. Cut the new worktree from that refreshed `main`** — the `EnterWorktree`
tool, or `git worktree add .claude/worktrees/<topic> -b <branch> origin/main`.
Cutting from a stale `main` is how a change arrives already behind and
conflicting.

**Then refresh from `main` periodically while the work is open**, and always
before asking for a merge:

```sh
git fetch origin && git rebase origin/main      # or merge, if the branch is shared
```

This is cheaper than it looks. `rust` and `frontend` compute a content-addressed
pass key, so rebasing over a `main` that moved only through paths the gate
ignores re-uses the existing green instead of paying for a new one.

What makes step 1 matter is disk, but **the cost is paid at the first build, not
at creation** — and that changes where to look. A worktree carries its own
`rust/target` only once something in it has compiled: a skills or docs session
that never runs `make check` stays around a gigabyte. Measured across 19 open
worktrees: **83GB in total, concentrated in one 24GB outlier that had built,**
while most carried no `rust/target` at all. The primary checkout's own
`rust/target` was **132GB** — larger than every worktree combined.

So the worktree count is a poor proxy for the problem. `--size` on the audit
names the expensive ones, and if the disk is genuinely tight the primary
checkout's `target` is the first place to look, not the worktree list. A full
disk surfaces as `linking with cc failed`, exactly the misleading CI symptom. A
fresh worktree also starts cold, so its first `make check` is a full build —
which is both the cost of isolation and the moment a worktree starts being
expensive to keep.

Still check `git branch --show-current` and `git rev-parse HEAD` in the same tool
call as any commit, and stage by explicit path. A worktree makes the shared-clone
collision unlikely, not impossible.

**`pkill` and `killall` are denied in `.claude/settings.json`, and the reason is
other sessions.** Every session runs the identical `make check`, so the command
line cannot tell yours from theirs — only the PID can, and the worktree path
never appears in a process's arguments at all (it is just the cwd, so
`pgrep -f '<worktree>.*make check'` matches nothing even while the run is live).
A `pkill -f 'make check'` therefore kills whatever the rest of the fleet is
doing: it happened, and it cost a peer session a full cold build. Capture the
PID when you start anything long and `kill` that.

And **never rebase, switch or stash while that session's own gate is running.**
The branch-switch case above makes the gate report artifacts; a rebase that
pulls in a dependency bump is worse, because the in-flight compile is then
running against a lockfile that no longer exists and its result means nothing
either way.

## Layout

The backend is the Rust workspace under `rust/`; the Python/dbt
implementation has been removed. **The cargo workspace root is `rust/`, not the
repo root** — bare `cargo` commands must run from `rust/`, which is why the
Makefile targets all `cd rust` first.

- `rust/crates/` — engine, CLI, control plane (`er-server`), entity tooling
- `entities/` — one YAML per table; the source of truth for schema
- `frontend/` — standalone pnpm project with its own gate
- `infra/` — terraform + helm charts
- `.claude/skills/` — the agent skills, and the design records behind them

There is no `docs/` directory. Every design document now lives in the
`references/` of the skill that owns its subject — the engine architecture and
runbook under `er-engine-ops`, the platform under `infra-platform`, the control
plane under `er-server-control-plane`, and so on — so a document is loaded with
the skill that would send you to it rather than sitting where nothing points at
it. `.claude/skills/er-engine-ops/references/architecture.md` is still the entry
point for the engine layer, and `DesignDoc.md` at the root remains the normative
specification. Superseded roadmaps and competitor research were deleted rather
than moved; they are in git history.

## Conventions worth knowing

**Schema changes go through `entities/`.** Every table is an `entity.yaml` and
every persistent SQL statement is a declared `.sql` + `.yaml` pair;
`er-entity gen` emits the Rust structs, DDL, Arrow schemas, and TypeScript
bindings. Never hand-edit a generated file. There are no migration files: the
catalog is compiled into `er-server` and every live store is reconciled to it
(additive changes applied and ledgered in `schema_migrations`, breaking ones
refused) at boot, at the start of every lake job, at provisioning and by the
deploy's per-tenant sweep. See the `entity-framework` skill and
`.claude/skills/entity-framework/references/entity-framework-design.md`.

**The entity conventions are a lint registry, not prose.** Every rule an
`entity.yaml` must hold lives in `rust/crates/er-entity-lint` as a value carrying
its own code, reasoning and worked examples:

```
er-entity lint                 # run the pass alone (verify runs it first)
er-entity lint --list          # the registry as a table
er-entity lint --explain V4    # one rule, with why it exists
```

`.claude/skills/entity-framework/references/entity-lint-rules.md` is **generated** from that registry and byte-compared
by `er-entity verify`, so the reference cannot drift from what is enforced. Adding
a rule is one file in `er-entity-lint/src/rules/`, one line in the registry, and
one fixture directory under `tests/fixtures/` — nothing else changes. Codes are
append-only; a retired rule leaves its code behind.

A consequence worth knowing: `Catalog::load_dir` now checks only structural
coherence. Anything about to write a generated artifact should load through
`er_entity_lint::load_linted` instead.

**Errors must not interpolate their own source.** A `#[error("...")]` string must
not include the `#[from]`/`#[source]` field (or a field named `source` —
thiserror treats that name as the source automatically). Reporters walk
`.source()` separately, so interpolating double-prints:

```rust
#[error("reading {path:?}: {source}")]   // wrong — double-prints
#[error("reading {path:?}")]             // right
```

This is enforced, not just documented: `er-errors/tests/display_no_source.rs`
scans every product source in the workspace and fails `make check` on a
reintroduction, naming the file, line and placeholder.

The flip side: `e.to_string()` alone now yields only the outermost message. Where
an error is flattened into a `String` rather than propagated — `er-server` does
this for job-failure text and HTTP bodies — walk the chain at that point:

- `anyhow::Error` → `format!("{e:#}")` (its alternate formatter walks the chain)
- a `thiserror` type → `er_server::errfmt::chain(&e)`

**The UI is strictly black and white.** Both themes are grayscale — no brand
accent, no colored chips or badges, no palette (`.claude/skills/frontend-design-system/references/frontend-design.md` §3.3).

This is enforced, not just documented: a `no-restricted-syntax` rule in
`frontend/eslint.config.mjs` rejects Tailwind palette utilities
(`bg-emerald-500`, `text-red-600`, …) in string and template literals, so
`make frontend` fails on a reintroduction. Use the semantic tokens in
`globals.css` — all chroma-0 in both themes — and signal state with weight,
scale, spacing or motion rather than hue. The `frontend-design-system` skill is
the standard: the token system, the channel to reach for instead of each hue,
and the one known exception (`--destructive` is still red). The techniques that
replaced the 47 colour utilities this rule originally found are also tabulated
in `.claude/skills/frontend-testing/references/frontend-testing.md`.

**All infrastructure is Terraform. Never generate CDK, CloudFormation or SAM.**
Terraform under `infra/terraform/` is the only infrastructure language here:
there is no `cdk.json`, no `template.yaml`, no `samconfig`, and that is a
decision rather than a gap. A CDK app is not a smaller first step — it is a
second, unmanaged source of truth for the same AWS account.

This is worth stating at the top level because the obvious tool for AWS work —
the `aws-core` plugin — is deliberately **not** installed. It ships CDK and
CloudFormation authoring skills against this repo's Terraform-only rule, and it
bundles its own `aws-mcp` server carrying neither `--read-only` nor a profile,
alongside the read-only one `.mcp.json` already declares. The AWS MCP server
that *is* installed still answers CDK and CloudFormation questions through its
documentation topics, so those idioms remain one search away. Reading
CloudFormation is fine — an EKS addon may surface as a stack AWS manages for
you. Adding to it is not. The `aws-account-operations` skill carries the
operating rules; the `infra-platform` skill carries the authoring ones.

**Shell scripts live under `scripts/`.** Every tracked `*.sh` or
shell-shebang file sits under `scripts/` (`scripts/ci/` for CI helpers,
`scripts/frontend-dev/` for the dev stack, `scripts/hooks/` for Claude Code
hooks), so shell is reused rather than rewritten. Enforced by
`scripts/ci/scripts-home-check.sh`, which runs in `make check` and in
rust.yml's `workflow pins (S9)` job.

**That rule has a blind spot, and it has its own gate.** A workflow's
`run:` block is shell with **no file**, so a file-level check cannot see it —
which is where it accumulated: seven copies of the build-cost report, four of
the disk reclaim, three helm installs each pinning `HELM_VERSION`
separately, and a 56-line `detect` step whose classification could not be
unit-tested because it was YAML. `scripts/ci/workflow-shell-check.sh` closes
it, and the rule is a **budget, not a ban**:

- more than **3 lines** of shell in a `run:` block → extract it;
- **any** control flow (`if`, `for`, `while`, `case`, a function), at any
  length → extract it, because that is a decision nothing can test in place.

Comments and blank lines are free, folded `run: >` scalars are one command
however long, and `gh aw`-generated workflows are skipped. Extract to
`scripts/ci/<name>.sh`, call it with `run: bash scripts/ci/<name>.sh`, and
pass values in through `env:` rather than interpolating `${{ }}` into shell.
It runs in `make check` and in the same ungated S9 job, with its self-test
first — a detector breaks by noticing nothing, which reads exactly like a
clean tree.

**Use `cargo add`** to add dependencies or features rather than editing
`Cargo.toml` by hand.

## Agent tooling

`make symposium` installs the repo-local [Symposium](https://github.com/symposium-dev/symposium)
toolchain: the `cargo-agents` binary into `.tools/bin`, its config/cache into
`.symposium`, and dependency-matched skills into `rust/.claude/skills/` for
Claude Code and `rust/.agents/skills/` for Codex. Nothing is written to `~` by
Symposium. Those nested skills load lazily — only once work enters `rust/` —
and the hooks Symposium writes under `rust/` are inert. The root
`.claude/settings.json` carries Claude Code's active SessionStart hook;
`scripts/codex-hooks-sync.sh` derives the Codex SessionStart hook from it and
installs that beside the Codex guard in `$CODEX_HOME/hooks.json`. Both call
`scripts/hooks/symposium-session-start.sh`, which points the hook payload's
`cwd` at `rust/`; changing only the shell directory leaves Symposium looking
at the repo root and silently skips its automatic sync.

### Other agents read this repo, and they read different filenames

Codex runs here too, and so does **Kimi Code CLI** (`kimi`) — a separate binary
from Moonshot, not to be confused with reaching Kimi K3 through `codex
--profile kimi`, which is the same `codex` binary and behaves as Codex does.
All of them discover skills at `.agents/skills/` and instructions at
`AGENTS.md`, and read **neither** `.claude/skills/` nor `CLAUDE.md`. Both are
authored once in the Claude Code locations and bridged across by symlinks:

```
.agents/skills/<name> -> ../../.claude/skills/<name>
AGENTS.md             -> CLAUDE.md
```

`make agent-bridge-check` enforces that, in both directions, and the gate also
rides ungated in `rust.yml`'s `skills` job — because a skills-only pull request
is exactly the shape `detect` skips, and so exactly the one that can orphan a
symlink. **Add a skill and you add its symlink**, or it works in one tool and
does not exist in the other.

**A bridged file still has to parse, and the three readers do not agree about
what parses.** Claude Code and Codex both tolerate a `: ` inside an unquoted
YAML plain scalar; Kimi Code CLI rejects the file and loads the skill as if it
were absent, logging a `SkillParseError` to `~/.kimi-code/logs/` that nothing
in this repo reads. Six skills were in exactly that state — bridged correctly,
and invisible in one of the three agents. `skills-check.sh`'s
`frontmatter_scalar_breaks_yaml` now fails that in `make skills-check`, so the
rule to know is that **a frontmatter value may not contain `: ` unless it is
quoted** — and for `description` specifically the fix is to reword, because the
generators extract it with a bare `sed` that would carry the quotes into their
output.

**The rest of `.claude/settings.json` is per-developer state in each agent's own
config home, so no repo file can supply it — but all of it except the plugin set
can be _derived_.** Run these once per machine, and again after the file they
read changes:

```sh
scripts/codex-mcp-sync.sh     # .mcp.json            -> codex mcp add
scripts/codex-hooks-sync.sh   # guard + Symposium SessionStart -> $CODEX_HOME/hooks.json
scripts/kimi-mcp-sync.sh      # .mcp.json            -> $KIMI_CODE_HOME/mcp.json
scripts/kimi-hooks-sync.sh    # the PreToolUse guard  -> $KIMI_CODE_HOME/config.toml
```

Each reads the Claude Code file and nothing else, so no two agents can be given
different lists by hand, and all three non-Claude installs share one guard at
`scripts/hooks/agent-pretooluse-guard.sh`. The plugin set is the one with no
equivalent at all — Codex refuses repository-scoped plugin installs.

**The Kimi pair is not a copy of the Codex pair**, for two measured reasons.
Kimi's MCP schema has no `type` key — it infers stdio from `command` and HTTP
from `url`, and needs `transport = "sse"` spelled out — so the servers are
translated rather than forwarded. And its hooks live in `[[hooks]]` tables
inside `config.toml` that accept **exactly** `event`, `matcher`, `command` and
`timeout`; a fifth field fails the whole config load, taking the provider and
model tables with it, which is why the sync brackets its block with marker
comments instead of tagging the table.

**One thing is better under Kimi and one is worse.** Better: there is no
hook-trust step, so an installed hook is live in the next session rather than
silently skipped until approved. Worse: hooks are fail-**open** by design — a
crash, a timeout or any non-2 exit lets the command through, which Kimi's own
documentation states and warns about. `ask` is an allow on both agents, so the
guard only ever emits `deny`. Treat it as a seatbelt over the IAM denies, never
as the thing standing between an agent and the account.

Two things about the hooks one are load-bearing, both measured against Codex
0.167.0:

- **A hook Codex has not trusted is skipped silently** — no warning, the command
  simply runs. Start `codex` interactively once and approve it (that persists a
  `trusted_hash`), or pass `--dangerously-bypass-hook-trust` every time. An
  installed-but-untrusted hook is worse than none, because it reads as cover.
- **`ask` is not a weaker `deny` there, it is an allow.** A hook returning `ask`
  is logged `PreToolUse Failed` and the command runs, so the guard's whole
  middle case — every mutating `aws`/`terraform`/`kubectl`/`helm` call — would
  be its widest hole. `scripts/codex-hooks-sync.sh` therefore installs the guard
  with `ER_GUARD_ASK_AS_DENY=1`, and under Codex those calls are **refused**
  rather than escalated. Run them yourself, or use a session that can prompt.

What does hold: a `deny` blocks even under
`--dangerously-bypass-approvals-and-sandbox`, which is the flag set these
sessions actually use. Sandbox bypass is not hook bypass.
[`.claude/skills/skill-writer/references/agent-skills.md`](.claude/skills/skill-writer/references/agent-skills.md)
carries the full decision matrix, the probe behind each row, and the one command
that shows what Codex actually loaded.

### Repo skills

First-party skills in `.claude/skills/`, organised by what you are doing rather
than by crate. **[`.claude/skills/skill-writer/references/agent-skills.md`](.claude/skills/skill-writer/references/agent-skills.md) is the
standard** — the authoring rules, why everything sits at the root, which
third-party skills and MCP servers are vetted in, and how the gate works.

The table below is **generated** from each skill's frontmatter by
`scripts/skills-check.sh regen`. Do not hand-edit it.

<!-- BEGIN GENERATED: skill-inventory -->
| Doing | Skill |
|---|---|
| adding or changing an endpoint's request or response shape | `api-contract` |
| asking what is deployed, investigating a bill | `aws-account-operations` |
| a CI job fails for reasons unrelated to the diff | `ci-and-supply-chain` |
| asked to pick up, claim, grab or start work on a ticket | `claim-ticket` |
| opening or attaching a lake, setting `partition_by` or `order_by` on a lake write | `ducklake-lakehouse` |
| a stage or query is slow, when proposing a partition | `ducklake-performance` |
| adding or changing a table, column, or SQL statement | `entity-framework` |
| running or scripting a pipeline stage, setting up a development lake | `er-engine-ops` |
| changing blocking or comparison config, training or retraining a model | `er-matching` |
| adding or reviewing an endpoint, handler | `er-server-control-plane` |
| adding or reviewing a route handler under `src/app/api` | `frontend-bff` |
| building or changing a table, list, chart | `frontend-data-ui` |
| building or restyling a component or page | `frontend-design-system` |
| adding or fixing a frontend test, when a Playwright or vitest run fails | `frontend-testing` |
| asked to check dev or prod health, hunt errors in the running system (user-invoked only) | `health-sweep` |
| editing Terraform or a Helm chart, writing a NodePool | `infra-platform` |
| asked to monitor, watch, babysit or land a pull request | `land-pr` |
| editing or adding a marketing page or its copy | `landing-page` |
| adding a span, metric, counter, gauge, log line or trace attribute | `observability` |
| invoking /pentest, /pentest <category>, /pentest live (user-invoked only) | `pentest` |
| reviewing a pull request, a branch or the working diff before merge (user-invoked only) | `pr-review` |
| asked to prioritize roadmap tickets or inspect the board | `project-board` |
| authoring or reviewing any Rust in this repo | `rust-conventions` |
| reviewing an endpoint or handler for SSRF/injection/tenant-isolation/auth/business-logic abuse | `security` |
| reviewing a skill before or after a change | `skill-audit` |
| adding a skill, when deciding whether a rule belongs in a skill or a hook (user-invoked only) | `skill-writer` |
| an E0xx diagnostic fails, when a statement will not plan | `sql-analysis` |
| building a faux product UI demo, a hero or feature-card illustration | `stageshow` |
<!-- END GENERATED: skill-inventory -->

Each carries an `evals.md` of hand-run scenarios. `make skills-check` enforces
the mechanical rules; counts and version numbers live in generated reference
blocks, never in prose, because a hand-written count in this repo goes stale
within weeks — including the count that used to sit in this very paragraph.

### Three general-purpose skills are overridden

A normal session loads skills that are each wrong for this repo in a specific
way. In every case the house skill states the supersession in its own body and
wins structurally, because root skills load at startup while nested and plugin
skills load later. The override is partial each time — only the conflicting
claim is superseded.

This is enforced, not just documented: `scripts/skills-check.sh overrides` (part
of `make check`) fails if a house skill stops naming the skill it supersedes, if
it is no longer a root skill, if a root skill collides with the overridden name,
or if this table and the script's registry drift apart. Load order is only half
the mechanism — the naming is the half that can rot, and when the gate was added
two of the three skills below were not naming their counterpart at all.

| Skill | Wrong here because | Overridden by |
|---|---|---|
| `rust-best-practice` (symposium-synced under `rust/.claude/skills/`, gitignored) | its `cargo fmt` + `cargo clippy` + `cargo test` finish line **skips `entities-check`** | `rust-conventions` |
| `frontend-design` | asks for a palette of 4–6 named hex values and deliberate aesthetic risk, against a product whose design doc forbids any palette | `frontend-design-system` |
| `dataviz` | assumes a charting library and a colour-bearing palette, against a codebase whose charts are hand-rolled DOM/CSS by decision | `frontend-data-ui` |

Which third-party plugins and MCP servers are vetted in, which are deliberately
out, and why, is recorded in
[`.claude/skills/skill-writer/references/agent-skills.md`](.claude/skills/skill-writer/references/agent-skills.md).
