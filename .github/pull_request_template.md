Closes #

<!-- ━━━ FOR THE AGENT WORKING THIS PULL REQUEST ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

LINE 1 IS THE TICKET LINK AND IT IS REQUIRED. Every pull request here closes a
board ticket: `Closes #<n>`, an issue in this repository. The `pull request
contract` check reads GitHub's own parsed closing references, not this text, so
a link that does not actually close the issue does not pass. No ticket for the
work? File one first — the board is the plan of record, and a pull request that
closes nothing moves no card.

THE MERGE IS NOT THE END OF THIS PULL REQUEST. The post-merge ledger below is
the deliverable's other half:

  * Work it after the merge, and tick each box IN THIS BODY with the evidence
    written inline. A merged pull request's body stays editable — that is why
    the record lives here and not in a session transcript nobody re-reads.
  * Three annotations, and only one of them leaves the box empty:
      - `n/a — <why>`              ticks; the item does not apply here
      - `deferred — #<ticket>`     ticks; real work, owned by a filed ticket
      - `blocked — <why>`          stays EMPTY, and goes in the report
    So an EMPTY box means the follow-up did not happen, and that is the only
    thing an empty box means. A `deferred` with no ticket number is reported as
    a loose deferral — file the ticket, then the annotation is true.
  * Add items to "Specific to this change" as you discover them, rather than
    leaving them in chat.

`/land-pr` owns the procedure and does not exit until the ledger is closed.

EVERYTHING ELSE STAYS SHORT. Two or three lines per section. The descriptive
material — plan, discoveries, measurements — goes in the collapsed Context
block at the bottom, where a reader opens it only if they want it.

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ -->

> **Post-merge is part of this pull request.** The ledger below records whether
> the follow-up work actually happened. `/land-pr` closes it.

## What

## Why

## Proof

<!-- The gate that was run and what it said: `make check`, `make frontend`,
     `make check-e2e`, or the tier this diff belongs in. Not "should pass". -->

## Risk & rollback

<!-- One line each. `git revert <merge-sha>` is an acceptable rollback. -->

<!-- BEGIN POST-MERGE -->

## Post-merge ledger

**Closed by:** _unclosed_

<!-- Replace that with who closed it, WHICH SESSION, on WHICH MACHINE, and when:
       session_ref="${CLAUDE_CODE_SESSION_ID:+Claude Code:$CLAUDE_CODE_SESSION_ID}"
       session_ref="${session_ref:-${CODEX_SESSION_ID:+Codex:$CODEX_SESSION_ID}}"
       session_ref="${session_ref:-unavailable}"
       printf 'agent · session `%s` on `%s` · %s\n' \
         "$session_ref" "$(hostname -s)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
     Use CLAUDE_CODE_SESSION_ID in Claude Code and CODEX_SESSION_ID in Codex.
     The transcript lives on that host. Reopen it with `claude --resume <id>`
     or `codex resume <id>`, respectively. If neither id is available, record
     `unavailable` rather than inventing one. -->


- [ ] **`main` is green with this change on it** — the merge SHA, and the run
      that proves it. A later head SHA with this commit as an ancestor counts;
      a cancelled run with zero jobs does not:
- [ ] **`images`** built, or correctly never ran because nothing that goes into
      an image changed:
- [ ] **`deploy`** converged — proved by a line the running process emitted, not
      by the image digest and not by the workflow conclusion:
- [ ] **the push-only surface no pull request can pre-test** (`deploy`, `drift`,
      `kind`'s nightly leg) is green, or knowingly red for a reason that is not
      this change:
- [ ] **the board card** reached `Done` through the automation rather than by
      hand, and the issue closed:
- [ ] **follow-ups** are open for everything deferred here — numbers:
- [ ] **what was learned is written down** where the next session will find it
      (a skill, a reference, memory) — or nothing new was learned:

### Specific to this change

<!-- Anything the list above cannot know: a one-off backfill to run, a flag to
     flip, a measurement to retake, a required check to add once this has
     reported, a doc to correct once it is deployed. Add `- [ ] …` items here;
     no items means nothing else is owed. -->

<!-- END POST-MERGE -->

<details>
<summary><b>Context</b> — plan, discoveries, measurements, files</summary>

### Plan

<!-- Ordered steps, kept current: tick them off, and record what changed them. -->

### Discoveries

<!-- Where the tree disagreed with the ticket. Probes, and what they returned. -->

### Measurements

<!-- Numbers, each with the command that produced it. -->

### Files

</details>
