#!/usr/bin/env bash
# agent-task.yml's last-resort checkpoint: commit and push whatever the agent
# left behind.
#
# WHAT IT CANNOT COVER, and why the prompt does not rely on it: it runs on
# failure, turn exhaustion and step timeout, but not on a hard node loss (a
# spot reclaim), which is why the agent is asked to push after every step
# rather than trusting this.
#
# `git add -A` IS CORRECT HERE AND ONLY HERE. CLAUDE.md forbids it because the
# repo's worktrees share a clone and staging everything commits another
# session's half-finished work. This runs on a disposable runner checkout that
# nothing else writes to, and the whole point is to lose nothing — an explicit
# pathspec would silently drop a file the agent created.
#
# The `--quiet` test keeps an empty checkpoint from becoming a commit, but the
# push runs either way: a resume needs the ref to exist even when this run
# produced nothing.
#
# Inputs (env): WIP (the checkpoint branch), RUN_ID.
# Run: WIP=agent-wip/123 RUN_ID=123 bash scripts/ci/agent-task-checkpoint.sh
set -euo pipefail

: "${WIP:?WIP must name the checkpoint branch}"
: "${RUN_ID:?RUN_ID must be set}"

git add -A
if ! git diff --cached --quiet; then
	git -c user.name="github-actions[bot]" \
		-c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
		commit -m "wip: agent checkpoint (run $RUN_ID)"
fi
git push origin "HEAD:refs/heads/$WIP"
