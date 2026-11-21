---
name: herdr
description: Controls Herdr terminal workspaces, tabs, panes and agents — explicit session selection, layout changes, fleet inspection and agent messaging. Use when the user explicitly mentions Herdr or asks to inspect or control its panes. Works inside or outside Herdr. For landing pull requests see land-pr; for repository skill maintenance see skill-writer.
disable-model-invocation: true
---

# Herdr

Use Herdr only for a user request involving Herdr. The installed CLI owns command
syntax; start with `herdr --help`, then print the relevant command group without
a subcommand (`herdr pane`, `herdr session`, `herdr agent`). Bare `herdr` launches
the UI; do not use it for discovery. Check `herdr status` before using a new
server feature, since client and server versions can differ.

## Select the session

Operating outside Herdr is supported. `HERDR_ENV` is context, not an access
requirement. This local rule supersedes the inside-Herdr prerequisite printed
by `herdr --skill`; do not restore that prerequisite when updating this skill.

Inside a managed pane, inherited session and pane context identifies the caller.
Outside it, run `herdr session list --json`, then carry `--session <name>` on
subsequent commands. Use the live response to select the intended session; ask
only if the user's target remains ambiguous. Use explicit pane IDs outside
Herdr, never `--current` or an assumed UI-focused pane.

For a saved SSH machine use `--machine <label-or-id>` consistently instead of
local session context. Discover its IDs there; local IDs cannot target it.

## Change the layout

Read `workspace list`, `tab list`, `pane list` and `pane layout` in the selected
session before moving or splitting anything. Reuse the pane the user named;
do not create another terminal with the same name to approximate a move.

Read command help for the installed version before a mutation. Preserve focus
with `--no-focus` where supported, unless changing focus is part of the request.
Keep the caller's working directory when creating a sibling pane. Read new IDs
from the response: IDs are opaque, and moving between workspaces changes them.

A request to keep a pane visible across tabs needs a persistent or pinned layout
feature, not just a split in one tab. Establish what the installed version
supports before promising persistence. Re-read the layout and configuration
after the change to verify both placement and the requested persistence scope.

## Inspect and coordinate agents

An agent is a process occupying a pane, not the pane itself. `idle` means
accepting input; it does not prove the task finished or a rate limit cleared.
Read the screen before sending input. `agent read` returns text, while list,
get and prompt return JSON with different envelopes.

For a fleet view, run from the repo root:

```sh
python3 scripts/herdr/fleet.py --session <name>
```

Without `--session`, the helper requires inherited pane context. It reads agent
screens and highlights stuck markers; it never sends input. Treat markers as
evidence to inspect, not a diagnosis or permission to restart an agent.

Use unique live agent names for repeated targeting. Names follow the occupant;
they disappear on exit or replacement. Do not prompt yourself. Do not inject
into an agent mid-task unless the user asked for that interruption. A shell
pane must be at its prompt before starting a command or agent in it.

For an authorized message, read
[references/inter-agent.md](references/inter-agent.md) before sending. Submission
and `--wait` are not proof that the requested turn completed: read the answer
and verify any claimed effect. Keep messages short and use an explicit timeout.

Panes can share a checkout. Inspect the target's working directory before asking
it to change git state; use a worktree for independent changes.

## Local automation and state

[references/local-automation.md](references/local-automation.md) describes the
optional quota probe and existing watcher. They are not part of ordinary layout
work. Do not start a watcher or send its recovery message unless requested.
Credentials, logs, locks and runtime state live outside the repository and skill
roots. Never copy an entire personal skill directory into Git.

## Verify before reporting success

Re-read the affected session's pane list and layout after any layout change.
For an agent task, read its screen and verify the requested effect. Never stop
or upgrade a live server merely to make a missing command work, and never close
an existing pane or workspace without the user's request.

For edits to this skill and its helpers, run `make skills-check`,
`make agent-bridge-check` and `python3 scripts/herdr/test_fleet.py`.
