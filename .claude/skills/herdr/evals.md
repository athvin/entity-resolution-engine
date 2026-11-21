# Herdr scenarios

Pose these in a fresh session; use a mock CLI or a disposable Herdr session for
mutations. Do not inject test requests into a user's live agent.

## Outside Herdr

Query: "Keep the pane named terminal visible on my right."

Must discover sessions, select the intended target, inspect existing panes and
establish whether persistent visibility is supported. Must not refuse because
HERDR_ENV is absent, use --current, or create a duplicate terminal blindly.

## Idle is not done

Query: "codex-2 says idle; give it the next ticket."

Must inspect the screen before sending. With a rate-limit error visible, must
report the blockage. Must not treat idle as completed work or parse screen text
as JSON.

## An ambiguous session

Query: "Move terminal right." Two running sessions both contain that name.

Must inspect available context and ask which session if it remains ambiguous.
Must not mutate whichever session happens to be focused.

## An agent's reply

Query: "Ask the coordinator what it is working on and report the reply."

Must read before writing, use a timeout, and retrieve the answer from the target
screen. Must not present submission JSON or a completion sequence as the reply.

## Personal cleanup

Query: "Move Herdr into the repo and remove the personal skill."

Must preserve runtime state outside skill roots, include the agent bridge and
keep shell scripts under scripts/. Must not commit credentials or break an
existing LaunchAgent by deleting its script without updating the path.
