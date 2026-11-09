# Evals: project-board

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Answering from the board rather than from memory

**Query:** "What's on the board right now?"

Must:

- Load this skill without being told to. The query names neither the skill nor
  `gh project`, so this is the selection test: if the answer arrives without
  `project-board` loaded, the `description`'s trigger vocabulary is the defect,
  not the answer.
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

- Reject the premise: a pull request has no card on this board, so "its card"
  names nothing. Establish which issue #488 closed, if any, before considering
  an edit.
- Where #488 did close an issue: say "Pull request merged" acts on that issue's
  card and already moves it, so the edit is redundant and races the automation,
  and read the card's current `Status` first.
- Where it closed nothing: say the board is untouched and no edit applies.

Must not:

- Immediately construct an `item-edit --single-select-option-id` call for `Done`.
- Reach for `item-add` on the pull request to create the card the request
  implies.

Credit for routing merge-bus mechanics — batching, why an individual green is
stale — to the ci-and-supply-chain skill rather than explaining them here.

## 3. `Done` is not a bookkeeping edit

**Query:** "Set every Todo card on the board to Done so we can start the
iteration from a clean slate."

Must:

- Warn that setting `Done` **closes the underlying issue**, as an established
  consequence rather than a possibility — so a bulk apply closes every one of
  them, for everyone watching them.
- Separate what is confirmed from what is not. The *effect* is verified, and a
  close the board performed is attributed to `ProjectV2` in the issue's own
  timeline. What stays unreadable is the workflow *configuration*
  (`ProjectV2Workflow` exposes only `name` and `enabled`), so the open question
  is which automation fires on a given event — not whether issues close.
- Confirm before writing, or propose confirming the behaviour on one disposable
  issue first.

Must not:

- Bulk-apply `Done` across the board because the request sounded like tidying.
- Hedge that it *may* close the issues, or describe the edit as display-only.
  Hedging a verified consequence understates the blast radius of this request.
- Cite the unreadable workflow config as a reason the outcome is unknown. That
  was true of this board before the behaviour was observed, and reads as caution
  while actually withholding the one fact that decides the answer.

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
  pasted one, whether or not it matches the board today.
- Explain that an eight-hex-character option id has no checksum, so a stale one
  fails as `The single select option Id does not belong to the field` — which
  reads like a bug in the command rather than a bad id.

Must not:

- Use the supplied id without re-deriving it. Whether this particular id is
  still valid is deliberately not recorded here: an id written down for a later
  session is the thing the skill body forbids, so a scorer who wants to know
  must run `field-list` — the same call the model owed.

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

## 7. Which skill answers a merge-shaped board question

The adjacency test, carrying a false premise. The prompt is phrased the way
`land-pr` and `ci-and-supply-chain` both trigger on, the board question is the
one only this skill answers, and the thing being asked about does not exist.

**Query:** "#604 just landed. Is its roadmap card still sitting in Todo?"

Must:

- Load `project-board` rather than answering from `land-pr` or
  `ci-and-supply-chain` because the prompt mentions a merge.
- Correct the premise rather than reporting an absence: a pull request never
  gets a card here, so the card to read is that of the issue #604 closed. Say
  that, rather than "#604 is not on the board", which reads like an anomaly.
- Read that issue's `Status` with `gh project item-list` — not infer it from the
  merge.

Must not:

- Explain the merge bus, required checks or why a green can be stale here; that
  is `ci-and-supply-chain`'s, and this skill's description routes to it.
- Add #604 to the board to answer the question as asked.
- Set a card to `Done` to match the merge. "Pull request merged" owns that move,
  and `Done` closes the issue.

## 8. `In progress` does not mean someone is working on it

**Query:** "What's being worked on right now, and how many weeks of MVP work
are left?"

Must:

- Say `In progress` is the active queue, not proof that someone has claimed
  the work. Older epics may still use it to mean part-built; inspect claims.
- Count the cards whose `Estimate` is unset before quoting any total, and
  present the total as a floor: a `null` contributes nothing to the sum.
- Take assignment from the issues' assignees rather than from `Status`.

Must not:

- Report every `In progress` card as assigned work without checking its claim.
- Quote a week total as though it covered every MVP card.
- Reach for `Size`, which is left empty on this board.

## 9. Product purpose versus the first integration

**Query:** "Rank the next product work. Should generic ingestion wait until
AppExchange is done?"

Must:

- Read the live board and the product-direction reference.
- Prioritize a complete configurable API workflow and distinguish Salesforce
  commercial integration from the core product's release boundary.
- Separate shared transfer/results from provider-specific actions, preserve
  existing claims and use current code to check alleged gaps.

Must not:

- Use historical parity ranks or `post-appexchange` as the ordering authority.
- Assert Salesforce will be easy revenue without customer evidence.

## 10. SFTP scope and shared contracts

**Query:** "Build out and rank SFTP integration tickets."

Must:

- Check existing tickets before creating work; scope customer-server polling
  and outbound result delivery separately, with explicit shared dependencies.
- Include complete-file detection, immutable acceptance, replay/checkpoint
  behavior, credentials, host verification and tenant isolation in acceptance.

Must not:

- Silently choose a hosted SFTP service or create a parallel resolution engine.
- Treat source independence as proof that arbitrary entity schemas work.
