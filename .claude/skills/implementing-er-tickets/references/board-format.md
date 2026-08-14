# Ticket contract and the board CLI

## Contents
- Ticket file layout
- Frontmatter fields
- Body sections
- The `board.py` subcommands
- What READY means

## Ticket file layout

One file per ticket at `docs/implementation/ER-<NNN>-<kebab-slug>.md`. The filename must start with
the `id`. `board.py validate` fails on any mismatch.

## Frontmatter fields

Machine-owned. `board.py` writes all of them; you never edit them by hand.

| Field | Meaning |
|---|---|
| `id` | `ER-NNN`. Immutable. |
| `title` | One line. Becomes the commit subject after the id. |
| `milestone` | `M0`–`M6`. Primary sort key for selection. |
| `status` | `todo` \| `in_progress` \| `blocked` \| `done` |
| `kind` | `code` (default) \| `spec-amendment` \| `docs` \| `fixture` |
| `size` | `S` \| `M` \| `L` |
| `gates` | `fast` \| `full`. A floor, not a ceiling — touching `src/er/`, `dbt/`, `docker/`, `fixtures/` or `tests/integration/` escalates to `full` regardless. |
| `depends_on` | Ticket ids that must be `done` first. |
| `spec_refs` | `DesignDoc.md` anchor ids (`s4-3`) this ticket implements. Validated against the real anchors. |
| `gap_refs` | Gap-report entries this closes (`B3`, `M12`). Provenance only. |
| `provides` | Symbols/modules this ticket creates. Feeds `INTERFACES.md` on completion. |
| `consumes` | Symbols from earlier tickets this one uses. |
| `owns` | Paths only this ticket may create. `plan-check` rejects another unfinished ticket planning them. |
| `protected_paths` | Paths this ticket must NOT modify — typically the failing test it must make pass. |
| `extra_paths` | Paths outside the normal envelope this ticket is permitted to touch (e.g. `.github/**`). |
| `attempts` | Incremented at `claim`. At 3, `claim` auto-blocks. |
| `verify` | The single command that fails before the ticket and passes after. |
| `branch`, `commit`, `spec_sha`, `updated_at` | Audit trail. |

Every scalar that can contain `:` or `#` is double-quoted on disk. `verify` and each `spec_refs`
element must be quoted or `validate` fails — an unquoted `#` is a YAML comment and would silently
truncate the command.

## Body sections

In this order. `validate` requires the acceptance-criteria numbering and the Definition of Done.

```markdown
## Description
## Scope
### In scope / ### Out of scope
## Design decisions applied
## Acceptance criteria
- [ ] AC1: <falsifiable assertion about observable behaviour>
- [ ] AC2: ...
## Tests
## Verification
## Definition of Done
## Blocker log        <- append-only, written by board.py
```

Acceptance criteria **must** use the `- [ ] ACn:` prefix: `plan-check` asserts every one is covered by
a planned file, and cannot do that without ids.

## Subcommands

`board.py --help` is authoritative; CI asserts this table matches it.

| Command | Effect | Notable exits |
|---|---|---|
| `validate [dir] [--strict]` | Schema, filename/id match, enum values, acyclic deps, dangling deps, duplicate ids, quoting, spec-anchor resolution. `--strict` also checks git reachability. | 1 on any defect |
| `next [--json]` | The highest-priority READY ticket. | 10 none ready, 1 board busy |
| `status [--json]` | Counts overall and per milestone. | |
| `show <ID> [--json]` | One ticket. | |
| `verify-cmd <ID>` | Prints the verify command. | |
| `claim <ID> [--session S]` | `todo` → `in_progress`, `attempts += 1`, records branch and spec hash, commits the board write. | 1 refused |
| `plan-check <plan> --ticket <ID>` | Validates a change plan before any code exists. | 1 on any plan defect |
| `complete <ID> --commit <SHA>` | `in_progress` → `done`. Requires: commit exists, subject starts with the id, ≥1 file, clean tree, a gate receipt whose tree equals the commit's tree with all exit codes 0, and (fast-scope) an independent verify re-run. | 1 refused, 3 dirty tree |
| `block <ID> --class C --failing-command … --assertion … --smallest-fix … [--log F]` | Appends a blocker entry and stops work. Returns you to `main`. `--class environment` restores `todo` and refunds the attempt. | 2 if the note is too thin |
| `unblock <ID> [--reset-attempts]` | `blocked` → `todo`. Human action. | |
| `release <ID>` | `in_progress` → `todo` after an interruption. | |
| `interfaces` | Regenerates `docs/implementation/INTERFACES.md`. | |
| `audit` | Detects board/git drift. | 1 on drift |

## What READY means

A ticket is READY when `status == todo`, `attempts < 3`, and **every** `depends_on` ticket is `done`.

`next` orders READY tickets by `milestone ASC, unblocks DESC, attempts ASC, size ASC, id ASC`.
`unblocks` outranks `attempts` deliberately: ordering by attempts first starves exactly the
high-fan-out tickets whose completion makes the rest of the board workable.
