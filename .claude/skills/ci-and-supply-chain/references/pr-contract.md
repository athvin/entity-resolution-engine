# The pull request contract: exemptions, promotion, and the rollup it exposed

Why `pr-contract.yml` is shaped the way it is, what it deliberately does not
check, and the two things its promotion to a required context cost care over.
Reached from [`../SKILL.md`](../SKILL.md).

## Contents

- [What it asserts, and what it refuses to](#what-it-asserts-and-what-it-refuses-to)
- [The exemptions](#the-exemptions)
- [Promoting a check to required](#promoting-a-check-to-required)
- [One SHA, several check-runs of one name](#one-sha-several-check-runs-of-one-name)

## What it asserts, and what it refuses to

Two clauses, both about the pull request **body** rather than the tree: that it
closes an issue in this repository, and that it carries the
`<!-- BEGIN POST-MERGE -->` ledger from
[`../../../../.github/pull_request_template.md`](../../../../.github/pull_request_template.md).

The ticket clause reads GitHub's own `closingIssuesReferences` — the same parse
that actually closes the issue on merge — rather than regexing the body. A regex
would pass `Closes #<n>` written inside a code fence, or a keyword GitHub does
not honour ("Addresses"), and the card would then not move for exactly the
reason the check exists.

The ledger clause asserts the block is **present**, never that it is finished.
At pull-request time every box is legitimately empty, because the work it
records has not happened yet. Tying the gate to any more of the template's
wording would make a prose edit a merge blocker.

It is its own workflow because it needs `types: [… edited …]`: adding
`Closes #<n>` to a description must turn the check green without a push, and
putting `edited` on `rust.yml` would re-run the backend gate on every typo fix.
It carries no `paths:` filter, for the usual reason.

The decision logic is pure and lives in `scripts/ci/pr-contract.sh`, so
`bash scripts/ci/pr-contract-test.sh` exercises it over fixtures with no
network — including the case that **the committed template passes its own
gate**, which is how a marker dropped from the template gets caught before it
fails every pull request opened afterwards. The job runs that self-test
**before** the check: a classifier that has stopped noticing anything passes
everything, which is indistinguishable from a clean tree.

## The exemptions

| Exempt | Why |
|---|---|
| a `merge-bus/*` head branch | a bus PR closes no issue by design, and `merge-bus-verdict.sh` reads its `statusCheckRollup` — a failing context there would stop **every** batch from landing |
| Dependabot | a bump has no ticket |
| a title starting `Revert` | `/land-pr` reverts to get `main` green; bookkeeping must not stand in front of that |
| the `no-ticket` label | the human override, matched on the whole name (`no-ticket-yet` is a different label) |

The first is load-bearing rather than a convenience, and the unit test for it is
the one worth keeping: a false failure there is worse than the gate not
existing.

## Promoting a check to required

Branch protection is repository configuration rather than a file here, so adding
a context is **never part of a pull request**. The sequence that made this one
safe, worth repeating for the next gate:

1. let the check report on real pull requests first;
2. observe it **failing** on one — a gate is not trusted until it has been seen
   red;
3. confirm no open pull request would be blocked, by asking the classifier per
   pull request rather than assuming;
4. then append the context.

Step 3 is the one with teeth. A context made required before that sweep blocks
every open pull request at once, and the two open here had bodies predating the
template — one was fixed by the session that owned it after being notified, and
the other had been opened from the new template already.

Two mechanics cost care:

- **The `required_status_checks` endpoint replaces the list** rather than
  appending to it. Read the existing contexts, keep each one's `app_id`, append,
  and re-send — dropping one silently removes a gate. Assert the old set is
  preserved before the `PATCH`.
- **`mergeStateStatus` cannot tell you what blocks a pull request.** Both open
  ones read `BLOCKED` immediately after the change and neither was blocked by
  the new context; each was waiting on its own Rust jobs. What answers it is
  diffing the satisfied set against the required set —
  [`polling.md`](../../land-pr/references/polling.md) carries that recipe.

## One SHA, several check-runs of one name

This is the first check here that can report **more than once per head SHA**,
because a body edit re-triggers it without a push.

Measured on #809: three body edits left **four** check-runs named
`pull request contract` on one SHA, and `statusCheckRollup` returns *all of
them* — including a `FAILURE` that a later run had already healed.

- Branch protection reads the **latest**, so merging is unaffected.
- `gh pr checks`, and anything summing the rollup, will show a red that no
  longer exists.
- `polling.md`'s recipe is already immune: `group_by(.name) |
  map(max_by(.started_at))`. That line is load-bearing rather than tidy.
- `merge-bus-verdict.sh` does **not** take the latest, but is not exposed — the
  bus pull request is exempt, so its contract check reports success and a healed
  failure cannot arise there.
