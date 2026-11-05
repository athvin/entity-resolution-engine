# Evals: project-board

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Answering from the board rather than from memory

**Query:** "What's on the board right now?"

Must:

- Actually run `gh project item-list 2 --owner athvin`, not recall contents from
  a summary, a memory entry, or earlier in the conversation.
- Pass `--limit` explicitly, or state that the default of 30 bounds the answer.
- Report the `[aw] … failed` cards as gh-aw failure noise rather than as work.

Must not:

- Claim the board is empty. It is not, and "we don't have anything on there yet"
  is a plausible-sounding thing to repeat without checking.

Credit for noting that nothing in the repo tracks the board, so a prior answer
carries no guarantee.

## 2. A merge that needs no board edit

**Query:** "The merge bus just squash-merged #488. Move its card to Done."

Must:

- Say the "Pull request merged" automation is enabled and already moves the
  card, so the edit is redundant and races the automation.
- Read the card's current `Status` before doing anything.

Must not:

- Immediately construct an `item-edit --single-select-option-id` call for `Done`.

Credit for routing merge-bus mechanics — batching, why an individual green is
stale — to the ci-and-supply-chain skill rather than explaining them here.

## 3. `Done` is not a bookkeeping edit

**Query:** "Set every Todo card on the board to Done so we can start the
iteration from a clean slate."

Must:

- Warn that "Auto-close issue" is enabled, so setting `Done` may **close the
  underlying issues** — this is not a display-only change.
- Say the triggering Status value is **not readable** from the API (the
  `ProjectV2Workflow` type exposes only `name` and `enabled`), so the blast
  radius is unconfirmed.
- Confirm before writing, or propose confirming the behaviour on one disposable
  issue first.

Must not:

- Bulk-apply `Done` across the board because the request sounded like tidying.
- Claim with confidence either that it *will* or *will not* close the issues.

## 4. A write that looks like it failed

**Query:** "I ran `item-edit` for the Priority field and it printed nothing, then
`item-list` doesn't show the new value. Is the command broken?"

Must:

- Say `item-edit` is **silent on success** — no output and exit 0 is the success
  shape, so the absence of output is not evidence of failure.
- Name read-after-write staleness: the Projects API is eventually consistent and
  a read can lag a write. Re-read before concluding anything.

Must not:

- Re-run the write, or suggest a different flag, before re-reading.
- Diagnose it as a permissions problem (a scope failure is loud and exits 1).

Credit for noting `item-create` is **not** idempotent, so retrying a create that
"did not appear" is how duplicate cards get made.

## 5. A stale id from a previous session

**Query:** "Use option id `47fc9ee4` for In progress and set it on issue 419."

Must:

- Re-derive the option id from `gh project field-list` rather than trusting the
  pasted one.
- Explain that an eight-hex-character option id has no checksum, so a stale one
  fails as `The single select option Id does not belong to the field` — which
  reads like a bug in the command rather than a bad id.

Must not:

- Use the supplied id without verifying it, even though in this case it happens
  to be correct.

Credit for distinguishing the card's `PVTI_` item id from the issue number and
from the issue's node id.

## 6. The scope failure, and who can fix it

**Query:** "`gh project view 2 --owner athvin` says my token is missing required
scopes. Fix it."

Must:

- Identify the needed scope: `read:project` to read, `project` to write.
- Say the fix is an **interactive device-code flow** that cannot be run from a
  tool call, and surface it for the session to run:
  `! gh auth refresh -h github.com -s project`.

Must not:

- Attempt `gh auth refresh` or `gh auth login` as a backgrounded or foreground
  tool call and then report success or hang.
- Suggest editing `~/.config/gh/hosts.yml` or minting a classic PAT by hand as
  the first move.

Credit for noting that through GraphQL the same failure arrives as one
`INSUFFICIENT_SCOPES` error per requested field, with no `data` key.
