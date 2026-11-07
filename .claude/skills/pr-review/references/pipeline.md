# The run

The mechanics. The judgment lives in the four dimension checklists; this file is
only about getting the right context to the right agent and not accepting its
word for anything.

## Contents

- [0. Eligibility, twice](#0-eligibility-twice)
- [1. Gather](#1-gather)
- [2. Fan out](#2-fan-out)
- [3. Verify](#3-verify)
- [4. Filter](#4-filter)
- [5. Post, only if asked](#5-post-only-if-asked)

## 0. Eligibility, twice

Stop before spending anything when the diff is closed, draft, bot-authored
(Dependabot, the merge bus), or generated-only — a lockfile bump, a regenerated
`openapi.json`, a regenerated skill reference. Run the same check again
immediately before posting, because the head may have moved while the reviewers
ran and a review posted against a stale SHA cites lines that no longer exist.

Two readings to get right at this step:

- **`DIRTY` runs zero workflows and reports "no checks reported".** That is not
  green and it is not a reason to skip the review either — the diff is still
  reviewable, the merge is not. Say so in the verdict rather than silently
  reviewing a branch that cannot land.
- **`gh pr checks` reports a PR green that compiled nothing.** This skill never
  reads CI state to decide a finding, which sidesteps the whole problem. If you
  find yourself wanting to, that is a `land-pr` job.

## 1. Gather

```sh
# Local branch
BASE=$(git merge-base origin/main HEAD); HEAD=$(git rev-parse HEAD)

# A pull request — take the SHAs from the PR, never from a local guess, and
# never predict a number
gh pr view <n> --json number,title,body,baseRefOid,headRefOid,isDraft,state,author,mergeable

git diff --stat "$BASE..$HEAD"
git diff --name-only "$BASE..$HEAD"
git log --no-decorate --oneline "$BASE..$HEAD"
```

Then collect, as **paths rather than contents** — the reviewers read what they
need:

- the root `CLAUDE.md`, plus any `CLAUDE.md` in a directory on a parent path of
  a changed file. A rule applies only to files under its own directory.
- this skill's references, **read at `$BASE`**:
  `git show "$BASE:.claude/skills/pr-review/references/<file>"`. A diff that
  edits a checklist does not get to be judged by its edited version. When the
  diff *is* a change to this skill, review it as an ordinary diff and say which
  version you used.
- the PR body's stated intent, and the linked issue if there is one.

Route dimensions by changed path. A dimension whose paths the diff does not
touch does not get a reviewer — this is where most of the cost is saved:

| Changed | Dimensions |
|---|---|
| `rust/crates/**/*.rs` | instrumentation, tier, reuse |
| `entities/**`, `**/*.sql` | sql, reuse |
| `rust/crates/er-server/src/dags/**` | tier (always), instrumentation |
| `frontend/src/**` | instrumentation (the action vocabulary), reuse |
| `infra/**`, `.github/workflows/**` | reuse (the shell-extraction budget) |
| `*.md`, `.claude/**` | none of the four — review as prose, briefly |

## 2. Fan out

One `pr-reviewer` subagent per routed dimension, in parallel, in a single
message so they actually run concurrently. Each gets **only**: its dimension,
its checklist path, `not-a-finding.md`'s path, `BASE`, `HEAD`, the changed-file
list narrowed to its paths, the stated intent, and the `CLAUDE.md` paths.

Never pass this session's history. The reviewer is judging the work product; the
author's reasoning about it — and yours — is exactly the anchor to withhold.

Large diffs, over roughly a thousand changed lines: split **by directory across
reviewers of the same dimension**, and have each review in passes and say so.
Reviewers do not spawn sub-reviewers; a recursive fan-out loses the dimension
boundary that makes the findings checkable.

## 3. Verify

One `pr-verifier` per finding, in parallel, fresh context. The reviewer that
produced a finding never scores it — that is the single structural property the
whole pipeline rests on, and the tool restriction on both agents is what makes
it more than an instruction.

The rubric is in `SKILL.md` and in the verifier's own definition, worded
identically on purpose. Hand it over verbatim; a paraphrase scores higher and
the threshold stops meaning anything.

## 4. Filter

- `>= 80` → **Important**.
- `50`–`79` → **Nit**, at most three, and only where a rule backs it. No rule,
  no nit.
- `0` because pre-existing **and** severe → **Pre-existing**, at most three,
  never blocking, never phrased as the author's doing.
- Everything else is dropped silently. Do not report a dropped count; it reads
  as "there were really nine problems" and invites a request to see them.

Then apply `not-a-finding.md` once more against the survivors. It is cheap, and
a reviewer under instruction still drifts toward the thing it was built to find.

## 5. Post, only if asked

`--comment` and nothing less. Without it the review is terminal output, which is
the right default for a branch that is still moving.

- Line-specific findings as inline review comments
  (`mcp__github_inline_comment__create_inline_comment` in CI, `gh api` review
  comments locally); the summary as one `gh pr comment`.
- Submit as **COMMENT**. Never `APPROVE`, never `REQUEST_CHANGES` — the verdict
  belongs in the text, where a human can disagree with it, not in a state that
  changes what the branch can do.
- Re-run the step 0 eligibility check first. A head that moved invalidates every
  line number in the report.
- One review per head SHA. Finding the same thing twice on the same commit is
  how a reviewer gets muted.
