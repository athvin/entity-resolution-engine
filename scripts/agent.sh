# shellcheck shell=bash
# Laptop-side helpers for .github/workflows/agent-task.yml. Source, don't run:
#
#   echo 'source /path/to/entity-resolution-engine/scripts/agent.sh' >> ~/.bashrc
#
# Needs an authenticated `gh` with write access to the repository. Every command
# dispatches against `main`, the only ref the `agent` and `agent-publish`
# environments accept, so a local feature branch checked out here does not
# matter.
#
#   agent "Add retry with backoff to the lake attach in er-lake"
#   AGENT_MODEL=opus AGENT_MAX_TURNS=120 agent "…"
#   agent-file specs/entity-merge-threshold.md
#   agent-resume 1234567890 "Address the review on the PR"
#   agents          # recent runs
#   agent-prs       # open agent pull requests
#   agent-watch     # follow the newest run's log
#
# The task id printed by `agent` is the run id of the dispatch. It names the
# branches (agent-wip/<id>, agent/<id>) and is what `agent-resume` takes.

_agent_wf=agent-task.yml

_agent_dispatch() {
	local -a args=(--ref main -f "prompt=$1")
	[ -n "${2:-}" ] && args+=(-f "resume=$2")
	[ -n "${AGENT_MODEL:-}" ] && args+=(-f "model=$AGENT_MODEL")
	[ -n "${AGENT_MAX_TURNS:-}" ] && args+=(-f "max_turns=$AGENT_MAX_TURNS")
	gh workflow run "$_agent_wf" "${args[@]}" || return
	# `gh workflow run` does not return the run it created. The newest
	# dispatch by this user is it, give or take a concurrent dispatch.
	sleep 4
	gh run list --workflow "$_agent_wf" --event workflow_dispatch \
		--user "$(gh api user --jq .login)" --limit 1 \
		--json databaseId,url \
		--jq '.[] | "task \(.databaseId)  \(.url)"'
}

agent() {
	[ $# -gt 0 ] || {
		echo 'usage: agent "<task>"' >&2
		return 2
	}
	_agent_dispatch "$*"
}

agent-file() {
	[ -f "${1:-}" ] || {
		echo "usage: agent-file <spec.md>" >&2
		return 2
	}
	# workflow_dispatch caps the whole payload at 65,535 characters.
	local size
	size=$(wc -c <"$1")
	if [ "$size" -gt 60000 ]; then
		echo "agent-file: $1 is $size bytes; dispatch inputs cap at ~64 KB. Commit the spec and point the prompt at its path instead." >&2
		return 2
	fi
	_agent_dispatch "$(cat "$1")"
}

agent-resume() {
	[ -n "${1:-}" ] || {
		echo 'usage: agent-resume <task id> ["what to do next"]' >&2
		return 2
	}
	_agent_dispatch "${2:-Continue the task and finish it.}" "$1"
}

agents() {
	gh run list --workflow "$_agent_wf" --limit "${1:-20}"
}

agent-prs() {
	gh pr list --state open --limit 100 \
		--json number,title,headRefName,isDraft,url \
		--jq '.[] | select(.headRefName | startswith("agent/"))
		      | "#\(.number)\(if .isDraft then " [draft]" else "" end)  \(.title)  \(.url)"'
}

agent-watch() {
	local id="${1:-}"
	[ -n "$id" ] || id=$(gh run list --workflow "$_agent_wf" --limit 1 --json databaseId --jq '.[0].databaseId')
	gh run watch "$id"
}
