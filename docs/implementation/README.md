# The implementation board

One markdown file per unit of work: `ER-<NNN>-<slug>.md`. `BOARD.md` is the generated overview;
`INTERFACES.md` lists what completed tickets provide.

This directory must exist in a fresh clone — git does not track empty directories, which is why this
file is committed.

## Reading a ticket

The YAML frontmatter is **machine-owned**. `scripts/board.py` writes every field; editing it by hand
desynchronises the board from git and `board.py audit` will flag it. The markdown body below the
frontmatter is human-authored and is where the actual requirement lives.

The field that matters most is `verify`: a single command that fails before the ticket and passes
after. If a ticket does not have one, it is mis-scoped.

## Working the board by hand

```bash
python3 scripts/board.py status              # where is the build
python3 scripts/board.py next --json         # what is workable now
python3 scripts/board.py show ER-013         # one ticket, in full
python3 scripts/board.py validate --strict   # is the board internally consistent
```

To implement one ticket yourself in an interactive session, type `/implementing-er-tickets` (or
`/implementing-er-tickets ER-013` for a specific one). It follows the same protocol the driver uses.

## Running the loop

```bash
bash scripts/run-loop.sh --max-iterations 3   # bounded, start here
bash scripts/run-loop.sh                      # until the board is clear
touch .loop/STOP                              # stop after the current iteration
```

The driver runs one ticket per fresh `claude -p` context, then independently re-runs the full gate
ladder on merged `main` and quarantines the commit if it fails. Exit codes are documented at the top
of `scripts/run-loop.sh`; notably **0 means the board is complete** and **4 means it is stalled** with
blocked tickets — do not read a non-zero exit as "the loop broke".

## Unblocking

A blocked ticket carries a `## Blocker log` entry naming the failing command, the assertion, and the
smallest proposed fix. Apply the fix (or amend `DesignDoc.md` yourself, which the loop may not do),
then:

```bash
python3 scripts/board.py unblock ER-041 --reset-attempts
```

## Adding tickets

Tickets are seeded from the design review, not invented ad hoc. If you add one, it needs: a unique
id, complete `depends_on`, at least one falsifiable `- [ ] ACn:` acceptance criterion, a `verify`
command, and a `## Definition of Done`. `board.py validate` enforces all of that.
