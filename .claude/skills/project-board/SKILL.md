---
name: project-board
description: Explains how to read and write this project's GitHub Projects board — the `gh project` command surface for user-owned project #2 "Product Roadmap" and the `project` scope it requires, the five ID namespaces none of which may be hard-coded, the built-in automations that move cards unasked and the one that can close an issue, `item-list`'s filter and JSON key shapes, the one-field-per-invocation rule for `item-edit`, and the read-after-write staleness that makes a successful write look failed. Use when asked what is on the board or to add, triage or move a card, when sizing, prioritising or scheduling work into an iteration, when reconciling the board against issues or pull requests, when a `gh project` command fails on a scope or an ID, or when deciding whether a card needs moving at all. For required checks, the merge bus and stale greens see `ci-and-supply-chain`; for driving a pull request to green and watching main see `land-pr`; for claiming a card and starting work on it see `claim-ticket`.
---

# The project board

One board tracks this project: **user-owned project number 2, "Product
Roadmap"**, private, linked to `athvin/entity-resolution-engine`.

```
https://github.com/users/athvin/projects/2
gh project view 2 --owner athvin
```

Every command here takes `2 --owner athvin`. The owner is a **user**, not an
organisation — so `--owner` is a login, and org-scoped project commands and
docs do not apply.

## The board is live state, not a checked-in artifact

Nothing in this repository references the board: no workflow adds to it, no
generator derives from it, and `make skills-check` never contacts it. That is
deliberate — every generator in `scripts/skills-check.sh` is grep/awk/sed over
committed files so the gate runs in a bare container with no network, and a
board-derived reference would break that property.

The consequence runs the other way too: **nothing in the repo can tell when the
board has changed.** Read it before acting on it, and never answer a question
about board contents from memory or from earlier in a conversation.

## The scope trap

A `gh` token with `repo` and `workflow` still cannot see the board. Reads need
`read:project`; anything that writes needs `project`. The failure is specific
and worth recognising, because it looks like a malformed query rather than a
permissions problem:

```
gh project view 2 --owner athvin
# error: your authentication token is missing required scopes [read:project]
```

Through GraphQL it is noisier — one `INSUFFICIENT_SCOPES` error **per requested
field**, and `gh` prints a body with no `data` key while exiting non-zero:

```
The 'projectV2' field requires one of the following scopes: ['read:project']
```

The fix is an **interactive device-code flow**, so it cannot be run from a tool
call. Ask for it to be run in the session:

```
! gh auth refresh -h github.com -s project
```

Confirm with `gh auth status`, which lists the granted scopes.

## Five ID namespaces, and none of them belong in a file

| Prefix | What it identifies | Where it comes from |
|---|---|---|
| `PVT_` | the project itself | `gh project view 2 --owner athvin --format json` |
| `PVTI_` | an **item** — a card on the board | `gh project item-list` |
| `PVTF_` `PVTSSF_` `PVTIF_` | a **field** — plain, single-select, iteration | `gh project field-list` |
| short hex | a single-select **option** or an **iteration** | `field-list`, or the GraphQL iteration config |
| `DI_` | the draft-issue **content** behind a draft card | `item-create` output |

Two of these are routinely confused. A card's `PVTI_` item id is **not** the
issue's node id and **not** the issue number; a draft card has both a `PVTI_`
item id and a separate `DI_` content id, and `item-edit` wants the item id.

**Do not commit any of these, and do not carry one across sessions.** An option
id is eight hex characters with no checksum, so a stale one does not look stale
— it produces `The single select option Id does not belong to the field`, which
reads like a bug in the command. Derive them every time; the lookups are one
call each and are collected in
[references/recipes.md](references/recipes.md).

## The board moves without being asked

GitHub's built-in item automations are configured on the board, and **every one
of them was enabled when last probed**:

```
Auto-add to project          Auto-add sub-issues to project
Item added to project        Item closed
Pull request linked to issue Pull request merged
Auto-close issue
```

Probe the live list — `enabled` is board state, not a repo fact:

```sh
gh api graphql -f query='query{user(login:"athvin"){projectV2(number:2){
  workflows(first:30){nodes{name enabled}}}}}' \
  --jq '.data.user.projectV2.workflows.nodes[] | "\(.enabled)\t\(.name)"'
```

**Only `name` and `enabled` are readable.** The `ProjectV2Workflow` type exposes
nothing about what a workflow does or which Status value it targets — that
configuration is visible only in the web UI. So the names above are read for
their intent, and the intent is all the API will confirm.

What follows from them:

- **A new issue lands on the board by itself. A pull request does not.
  Verified, not inferred.** Every item on the board reads back as an `Issue`,
  and four pull requests merged in one sitting produced no card at all — so
  "Auto-add to project" is filtered to issues here. The filter itself is not
  readable (only `name` and `enabled` are), so probe the shape rather than
  trusting this line:

  ```sh
  gh project item-list 2 --owner athvin --limit 400 --format json \
    --jq '[.items[].content.type]|group_by(.)|map({(.[0]//"null"):length})|add'
  ```

  For an issue, `item-add` is therefore usually unnecessary. It is idempotent —
  re-adding an item already on the board returns the existing `PVTI_` id and
  creates no duplicate — so it is safe, just redundant. For a pull request it
  would add a card the conventions here do not want.
- **A new card arrives with `Status` already set.** A draft created through
  `item-create` reads back as `Todo` without `Status` ever being written, because
  "Item added to project" sets it. Do not follow a create with a Status edit.
- **A merged pull request moves the card of the issue it closes — it has no
  card of its own.** "Pull request linked to issue" and "Pull request merged"
  act on the linked issue, so a pull request closing nothing moves nothing, and
  the merge bus squash-merging a batch of such pull requests leaves the board
  untouched. Where a pull request does close an issue, moving that card by hand
  is duplicated work and races the automation.

  **The Status it writes is `Done`. Verified, not inferred.** Two merged pull
  requests that closed an issue each left that issue's card on `Done`, with the
  issue closing about two seconds behind the merge.

  What cannot be separated is *which* automation did it. A merged closing pull
  request also closes the issue, so "Pull request merged" and "Item closed"
  both fire, and the workflow configuration stays unreadable. The effect is the
  same either way — and the route is readable after the fact, from the issue's
  own timeline rather than from the board:

  ```sh
  gh api graphql -f query='query{repository(owner:"athvin",name:"entity-resolution-engine"){
    issue(number:589){timelineItems(itemTypes:[CLOSED_EVENT],last:1){
      nodes{... on ClosedEvent{closer{__typename}}}}}}}' \
    --jq '.data.repository.issue.timelineItems.nodes[0].closer.__typename'
  ```

  | `closer.__typename` | What closed the issue, and so moved the card |
  |---|---|
  | `PullRequest` | a merged pull request that referenced it |
  | `ProjectV2` | the board itself — "Auto-close issue", because someone set `Status` to `Done` |
  | absent on a `CLOSED` issue | closed directly, by a person or the CLI; "Item closed" still set `Done` |

  That middle row is the independent corroboration of the bullet below: when a
  card is set to `Done`, the close is attributed to `ProjectV2`, so the board is
  on record as the actor rather than it being inferred from timing.
- **`Status` → `Done` closes the underlying issue. Verified, not inferred.** An
  issue observed `OPEN`, then `Status` set to `Done` through `item-edit`, read
  back as `CLOSED / COMPLETED` within seconds. So `Done` is never a bookkeeping
  edit: it resolves the issue for everyone watching it. Setting it on work that
  is not finished is a wrong state change, not an untidy board.

  Use it deliberately — recording already-shipped work is the case where
  closing is the point. Do not use it to tidy, and do not bulk-apply it.

The general rule: **check whether an automation already did the work before
writing.** The board has more than one writer.

## Reading

```sh
# everything, with the custom fields
gh project item-list 2 --owner athvin --limit 200 --format json

# filtered, in Projects filter syntax
gh project item-list 2 --owner athvin --query 'status:"In progress"'
gh project item-list 2 --owner athvin --query '-status:Done'
```

About that output:

- **`--limit` defaults to 30.** A board larger than that is silently truncated,
  which reads as "that card is not on the board". Pass `--limit` explicitly on
  any query whose answer is a count or an absence.
- **`totalCount` does respect `--query`** — it is the filtered count, not the
  board total. A filter matching nothing gives `totalCount=0`, and that is a
  real zero rather than a broken filter.
- **Custom field values are keyed by the field's name, lowercased, spaces and
  all.** `Target date` becomes `"target date"`, so jq needs bracket form:

  ```sh
  --jq '.items[] | {title, est: .estimate, due: .["target date"]}'
  ```

  `iteration` comes back as an object (`title`, `startDate`, `iterationId`),
  not a string.
- **Parse it with `--jq`, not an external JSON parser.** Issue bodies reach the
  item list verbatim, so a body containing a raw control character produces
  output that `python3 -c 'json.load(...)'` rejects outright —
  `Invalid control character at: line 1 column 113`. `gh`'s built-in `--jq`
  tolerates it. The failure looks like a corrupt response rather than a
  strictness difference, and it only appears once some issue on the board
  happens to contain one, so it arrives long after the script was written.

For anything `item-list` flattens — a field's `dataType`, an iteration's
configuration, the automations — go to GraphQL. `projectV2(number:2)` accepts a
`field(name:"…")` lookup, which is the least brittle way to reach one field.

## Writing

| Doing | Command |
|---|---|
| put an existing issue on the board | `gh project item-add 2 --owner athvin --url <issue-url>` |
| create a card with no issue behind it | `gh project item-create 2 --owner athvin --title …` |
| set one field on one card | `gh project item-edit …` |
| take a card off the board | `item-archive` (keeps it, hidden) or `item-delete` (gone) |

`item-create` makes a **draft**. A draft has no assignees, labels, milestone or
repository, and no issue to close — so it is the right shape for a placeholder
and the wrong shape for work anyone else must find. Prefer an issue:
`gh issue create --project "Product Roadmap"` works because the board is linked
to the repo, and "Auto-add to project" would have done it anyway.

### `item-edit` is one field per invocation

```sh
gh project item-edit --id <PVTI_…> --project-id <PVT_…> \
  --field-id <PVTSSF_…> --single-select-option-id <hex>
```

- **`--project-id` is required** for every field edit, including on a draft.
  Omitting it prints `project-id must be provided`, dumps the usage block, and
  exits 1 — and the usage block is long enough that a piped `tail` hides the
  one line that matters.
- **One field value per invocation.** Setting four fields is four calls.
- **Success is silent.** No output, exit 0. The only confirmation is a read-back.
- The flag must match the field's `dataType`:

| `dataType` | Flag |
|---|---|
| `SINGLE_SELECT` | `--single-select-option-id <hex>` |
| `ITERATION` | `--iteration-id <hex>` |
| `NUMBER` | `--number <n>` |
| `DATE` | `--date YYYY-MM-DD` |
| `TEXT` | `--text "…"` |
| any | `--clear` |

A mismatch fails loudly rather than writing the wrong thing, which is the one
mercy here. The four messages worth recognising, all exit 1:

```
Did not receive a single select option Id to update a field of type single_select
Did not receive a number value to update a field of type number
The single select option Id does not belong to the field   # stale or guessed id
project-id must be provided
```

The third also appears when an option **name** is passed where an id belongs —
`--single-select-option-id "Todo"` is rejected with exactly that message, not
with a complaint about the name.

## Read-after-write is stale

The Projects API is eventually consistent, and the window is long enough to
mislead. Both directions have been observed:

- An item created moments earlier was **absent** from the next `item-list`.
- Immediately after a delete, `item-list` reported `totalCount=4` alongside
  three items — the count lagging the list, in one response.

So: **a read that disagrees with a write that exited 0 is the read being stale,
not the write having failed.** Re-read before concluding anything, and never
retry a create because the first one "did not appear" — that is how duplicates
get made. `item-add` is idempotent and `item-create` is not.

## Conventions here

The board is GitHub's stock "Product Roadmap" template. Most cards are issues
carrying one **class** label:

| Class | What it is | Also carries |
|---|---|---|
| `epic` | a DataGroomr parity epic, titled `E<n> — <name>` | `mvp` or `post-appexchange` |
| `docs-backlog` | open work found in `docs/` and verified against the code | `mvp` or `post-appexchange` |
| `ci-backlog` | CI, runner and merge-bus work | — |
| `obs-backlog` | observability work | — |
| `followup` | a small operational follow-up an investigation surfaced; not an epic and not a `docs/` finding | a release flag only when it gates one |
| `shipped` | a built capability documented in `docs/`, recorded so the board shows the product and not only its gaps | nothing — closed, so the release question is moot |

**This table is a snapshot and will go stale, because the board has several
writers.** Concurrent sessions file cards here: `ci-backlog` and `obs-backlog`
both arrived that way without this file being touched, and `obs-backlog` showed
up between writing the row above and verifying it. Some cards carry no class at all — auto-added `[aw]` failure
issues and anything opened straight on GitHub. So **derive the classes you are
reasoning about** rather than trusting the list:

```sh
gh issue list --state open --limit 200 --json labels \
  --jq '[.[].labels[].name]|group_by(.)|map({(.[0]):length})|add'
```

**A release's week total is only as complete as its estimates.** Cards arrive
from other writers with `Estimate` unset, and a `null` contributes nothing to a
sum — so a view that totals a release's weeks silently excludes them, and the
total it shows is a floor rather than an estimate. Nothing in the repo can
refresh that number, so do not quote one from this file or from a previous
session; count the cards missing an estimate first:

```sh
gh project item-list 2 --owner athvin --limit 400 --format json --jq '
  [.items[]|select(.status!="Done")|select(.labels|index("mvp"))] as $m
  | "estimated \([$m[]|select(.estimate)]|length) of \($m|length)"'
```

**`docs/` understates what is built, consistently.** The checkboxes, the
`frontend-design.md` §6.1 status column and its §7 gap list have all been
observed stale in the same direction: features marked `Roadmap` or `Gap` that
already have routes. A `docs/` item is a hypothesis until grepped against
`rust/crates/er-server/src` or the entity catalog. One stale row reached a
board card before this was caught.

The conventions below are practice rather than a property of the board, so
edit this section as it changes.

- **`Status`** is `Todo` / `In progress` / `Done`, and the automations own most
  of its movement. Treat a hand edit as the exception. `In progress` is the
  **active queue** — work deliberately slated to be taken on now. (Older epic
  cards used it to mean *part-built*, and some still read that way.) It does
  **not** say anyone is on a card: that is a *claim* — assignee plus an open
  draft pull request — which is the `claim-ticket` skill's procedure.
- **`Priority`** is `P0` / `P1` / `P2`. There is no P3 — the absence of a
  priority is the backlog, and that is load-bearing: the lowest-ranked epics
  carry no priority at all rather than a notional P2.
- **`Estimate`** carries **engineering-weeks remaining**, and **`Size` is left
  empty**. Both fields exist because the template ships both; two maintained
  estimates disagree within a week, so pick one. Estimate won because it sums —
  a board view totals a release's weeks, which `XS`–`XL` cannot.
- **The release flag is a repo label, not a board field.** Labels travel with
  the issue and work in `gh issue list --label mvp`; a board field would be
  invisible outside the board. The cost is that board views can filter on it
  but not group by it.
- **Stack rank lives in the issue body** (`stack rank N of 30`), not in a
  field. Issues were created in rank order so the default board order matches,
  but nothing enforces that — the body is the record.
- **`Iteration`** is a 14-day cycle starting Monday. The *current* iteration is
  the first entry of the field's `iterations` array — completed ones move to a
  separate `completedIterations` array, so index 0 is not a guess.
- **A card follows the pull request that closes its issue, not the reverse** —
  and only if one does; the pull request itself never has a card. The merge bus
  is what moves work to done; see the ci-and-supply-chain skill for why an
  individual pull request's green can be stale.
- **The `[aw] … failed` cards are not work.** They are failure issues from the
  gh-aw agentic workflows, which are `disabled_manually` and fail closed on a
  missing `COPILOT_GITHUB_TOKEN`. They arrived by "Auto-add to project". Closing
  them is housekeeping, not progress.

Copyable sequences for triage, bulk reads and reconciliation against issues:
[references/recipes.md](references/recipes.md).
