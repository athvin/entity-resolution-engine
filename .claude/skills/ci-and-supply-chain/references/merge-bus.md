# The merge bus

## Contents

- Why it exists, and why not GitHub's merge queue
- Eligibility: what rides and what is refused, with reasons
- The two invariants that make landing sound
- Scopes, and the one it must never be given
- The simulator, and what mutation-testing it found

`main`'s protection has **`strict: false`** — "require branches up to date
before merging" is off — so a PR may merge on a green it earned against a base
that has since moved. With five or six PRs open at once, four PRs each green
against an older `main` can all merge and leave `main` red, and no run ever
examined the combination. `merge-bus.yml` is that missing run: it batches the
PRs that are ready, opens a PR for the combination so the ordinary gate judges
it, and merges the members only if that goes green.

GitHub's **merge queue is this feature and cannot be enabled here** — it is
offered only for organization-owned repositories, public on any plan or private
on Enterprise Cloud, and this one is private and user-owned. Probed, not
assumed: `rulesets` is `[]` and `mergeQueue(branch:"main")` is `null`. The
self-hosted alternatives are worse rather than better: bors-ng is archived, and
rust-lang/bors and Prow's `tide` are long-running GitHub Apps needing a hosted
service, a webhook endpoint and an App key — more standing credential than a
workflow file, and each restates the required checks in bot config where they
can drift from protection silently.

Four properties worth knowing before changing it:

- **It opens a pull request, and that is why it cost no edits elsewhere.** The
  three required workflows trigger on bare `pull_request`, so a batch branch
  earns every context with correct `detect` behaviour. Wiring the native queue
  would have been *more* work: `merge_group` is not a `pull_request` event, so
  all three `detect` jobs would fall through to their fail-safe default and
  every merge-group build would run every gate at full cost.
- **Eligibility asks GitHub rather than restating the required set.** With the
  review requirement at zero, `mergeStateStatus == CLEAN` *is* "every required
  check is satisfied", so the bus enumerates no contexts and cannot drift from
  protection. `DIRTY` is rejected **by name**, because a conflicted PR runs zero
  workflows and reports no checks — any predicate phrased as "nothing is
  failing" reads that as green. Note `gh pr list` returns `UNKNOWN` and does not
  trigger the computation; only `gh pr view` does.
- **It tells a bad batch apart from a starved account.** Cause 0 above presents
  as `failure` on every required context, which the bus would otherwise report
  as "the batch went red" — sending someone to debug a diff that was never
  compiled. So it refuses to build a batch when none of the last five completed
  runs executed a step, and if capacity runs out mid-validation it reports
  `starved` instead of red: no `bus-failed` label, and no comment on the
  members, because during an outage every PR in the repository is equally stuck.
- **A PR touching `.github/workflows/` can never ride the bus.** GitHub refuses
  a push from an App-authenticated token that creates or updates anything under
  that path — *"refusing to allow a GitHub App to create or update workflow …
  without `workflows` permission"* — and **`workflows` is not a grantable
  `GITHUB_TOKEN` scope**, so no `permissions:` line fixes it. Only a PAT with
  the `workflow` scope could, which would hand a long-lived credential push
  rights over CI itself. The bus therefore skips such PRs at selection, saying
  so, and they are merged by hand. Found on the second live run, where
  everything else worked and only the push was rejected.
- **It needs `actions: read`.** A declared `permissions:` block sets every
  unnamed scope to `none`, and both the pre-flight and the verdict
  classification read `/actions/runs`. Omitting it is how the first live run
  died, with a 403 the stubbed simulator could never have produced.
- **It stops on red and does not bisect.** A failed batch merges nothing,
  comments the failing contexts on the bus PR and every member, and leaves the
  bus PR open because its branch is the only place the failing combination
  exists. A *conflict* is ejected instead of stopping the batch — that is found
  before any CI is spent and names the culprit exactly.
- **Two invariants make the landing sound.** `main` must still be at the SHA the
  batch was cut from, or the green no longer describes what merging would
  produce; and after the squashes, `main`'s **tree** must equal the tree the
  gate examined. The second exists because `state: MERGED` is not evidence —
  a stacked PR whose base branch is deleted is marked merged, with a real
  `mergeCommit`, while its code never reaches `main`. For the same reason the
  bus retargets stacked children onto `main` *before* merging their parent,
  which is the only time `gh pr edit --base` still works.

**It has a simulator, and you should run it.** `bash scripts/ci/merge-bus-test.sh`
executes the workflow's real step scripts with a runner's semantics for the
parts that carry bugs — per-step `$GITHUB_OUTPUT`, `${{ }}` in `env:`, `if:`
gating, a failed step halting the job — against a local bare repo with a
stubbed `gh`. Git operations are genuine; only the GitHub API is faked. It
exists because `workflow_dispatch` fires only for a workflow already on the
default branch, so the live path cannot be exercised before the thing is
merged, and because this is bash that merges to `main`.

Each scenario was **mutation-tested**: the matching guard was deleted from the
workflow and the suite confirmed to go red. That exercise is what found the one
blind spot worth knowing about — removing the land step's tree assertion left
every scenario green, because the tree always matched. The `lying` scenario now
simulates `gh pr merge` reporting success while landing nothing, which is the
real shape of the trap (a stacked PR marked `MERGED`, with a `mergeCommit`,
whose code never reached `main`). It is hand-run, like a skill's `evals.md`:
it needs python3 with PyYAML, and a required check may not be added here before
it has been observed failing.

`merge bus` **must never become a required status check**: it runs on
`workflow_dispatch` only, so it can never report on a PR, and a required
context that never reports blocks every PR forever. It is also the first
hand-written workflow here with `contents: write` and `pull-requests: write`
— it pushes a branch and merges PRs — and it adds no repository secret to get
them. It is deliberately **not** an admin, which tightens rather than loosens
things: every merge it makes must satisfy the required checks, where a human
running `gh pr merge --admin` bypasses all of them.

