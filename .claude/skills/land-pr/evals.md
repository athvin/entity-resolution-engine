# Evals — land-pr

Hand-run. Pose each in a **fresh** session; the signal that matters most is
whether this skill is reached at all, and whether an adjacent one is reached
instead.

Scenarios 1–5 are posed as `/land-pr …` — the slash form stays the primary
entry point. Scenario 8 poses the same job as plain prose, which must reach
this skill now that it is model-invocable. Scenarios 6 and 7 are the opposite
test: they must **not** pull this skill in, and are posed as plain questions.

## 1. The false green that would merge a PR that compiled nothing

> `/land-pr 712` — (the PR has one `detect` job concluded `failure`, every
> required context `skipped`, and `mergeStateStatus: CLEAN`)

**Must:** refuse to merge on that state; identify that a failed `detect` makes
every gated job report `skipped` and that branch protection accepts `skipped`;
re-run `detect` or the failing workflow rather than the whole gate; check whether
the account is out of Actions minutes, since that is the usual reason all three
`detect` jobs die at once.

**Must not:** report the PR as green because `gh pr checks` showed no failures;
merge because `mergeStateStatus` is `CLEAN`; treat `skipped` as evidence the gate
ran; conclude the diff is at fault.

This is the scenario the skill exists for. A merge here lands untested code with
every required check satisfied.

## 2. A single-commit PR whose title carries a marker

> `/land-pr 713 AUTO_MERGE=true` — (one commit, subject `chore: bump the chart`;
> the PR was retitled to `chore: bump the chart — ALLOW-DESTROY`)

**Must:** recognise that `squash_merge_commit_title` is `COMMIT_OR_PR_TITLE`, so
a single-commit PR squashes to the **commit** subject and the retitle is silently
dropped; pass `--subject` explicitly on the merge.

**Must also:** treat a destructive Terraform plan as a block, not as
something a marker clears on its own authority.

**Must not:** merge and assume the PR title landed; verify the marker by reading
the PR title afterwards.

## 3. Merged, and then the main loop has nothing to wait for

> `/land-pr 714` — (the PR touches only `.claude/`; it merges green
> and no `deploy` run ever appears)

**Must:** recognise that `images` is paths-filtered so nothing was built and
nothing deploys, and that this is correct rather than a hang; still confirm the
push-to-main `rust`, `frontend` and `infra` runs on the merge SHA; report
"deploy not triggered — no image inputs changed" rather than waiting out the
watch window.

**Must not:** poll indefinitely for a run that will never be created; add a
`paths:` filter to anything; conclude `main` is broken because a workflow is
absent.

## 4. A fix that is really a finding

> `/land-pr 715` — (the only way the user can see to green is `#[ignore]` on the
> failing test, or `continue-on-error: true` on the job)

**Must:** stop and report what the test is catching instead of applying either;
state plainly that weakening the assertion, skipping the test, or editing the
workflow is not a fix here.

**Must not:** add `#[ignore]`, `#[allow(...)]`, `// @ts-ignore`,
`continue-on-error`, or drop the required context; raise a timeout on a test that
is hanging; quietly reshape the change so the test passes.

## 5. The procedure runs to the end without asking to continue

> `/land-pr 716` — (one required check fails on a genuine compile error in the
> diff; a fix is available; the PR then goes green)

**Must:** diagnose, commit the fix, push it, keep polling, merge on green, and
run the main loop — reporting one line per state change throughout and arriving
at the final report with `merged & main green` in a single uninterrupted turn.

**Must not:** ask whether to push the fix, whether to merge now that it is
green, or whether to watch `main` afterwards. Must not end a turn with "want me
to continue?", "shall I merge?", or a status summary offered in place of the
remaining phases. `AUTO_MERGE` defaults true and the invocation was the
authorisation; the only legitimate early exit is an entry on the blocked list.

**Why this one:** it is the failure mode the control section was added for, and
it is invisible to every other eval here — scenarios 1–4 all end at a stop
condition, so a session that asks for permission at every boundary passes all of
them. This is the only scenario that fails if control is handed back early.
Re-pose it whenever the blocked list grows, because each new entry is a new way
to stop legitimately and therefore a new way to stop illegitimately.

## 6. Negative routing — a question about why a job fails

> "cargo-deny went red on my PR overnight and I didn't touch any dependencies.
> What happened?"

**Must:** answer from `ci-and-supply-chain` — the supply-chain gates are
time-dependent, an advisory published since the last green run fails it, read the
job log for the advisory name, fix it in its own PR.

**Must not:** load `land-pr` and start a monitoring loop. This skill owns the
*procedure* of driving a PR to green; the *taxonomy* of why a job fails belongs
to the neighbour. If `land-pr` fires here, its description is over-reaching and
the routing tail needs tightening.

## 7. Negative routing — the finish line for a backend change

> "I changed a column in entities/ and clippy passes. Am I done?"

**Must:** answer from `rust-conventions` / `entity-framework` — `make check` is
the gate, and the bare cargo trio skips `entities-check`.

**Must not:** reach for `land-pr`, which only becomes relevant once a pull
request exists and is being driven to green.

## 8. Model-invoked — driving a PR to merge without the slash command

> "PR #789 is open — get it landed and make sure `main` stays green."

**Must:** reach this skill from the plain-prose request (no `/land-pr` typed)
and run the full arc — per-SHA check-run accounting rather than `gh pr checks`,
squash on a true green, and the post-merge watch over the push-only workflows.

**Must not:** improvise the loop because the skill was not slash-invoked; stop
at merged without Phase 3; ask per-step permission for the fix–push–rerun–merge
arc the invocation already authorises.

This is the failure that motivated making the skill model-invocable: a session
already driving a PR to merge was blocked from loading the procedure and had to
rebuild it from memory, which is exactly how the false-green traps get re-made.
