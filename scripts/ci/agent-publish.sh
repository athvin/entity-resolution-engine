#!/usr/bin/env bash
# agent-task.yml's publish job: move agent/<task> to the checkpoint and open
# or update the PR. Runs with the App token (GH_TOKEN), never GITHUB_TOKEN.
#
# Inputs (env): GH_TOKEN (App token), REPO, TASK_ID, BASE, OUTCOME, RESULT
# (the agent's structured JSON, treated as data), TASK, RUN_URL.
set -euo pipefail
# Defence in depth: the agent job validated this, but this job holds
# the App token and re-checks anything that reaches a ref name.
printf '%s' "$TASK_ID" | grep -Eq '^[0-9]{1,20}$'
head="agent/$TASK_ID"

if ! sha=$(gh api "repos/$REPO/git/ref/heads/agent-wip/$TASK_ID" --jq .object.sha 2>/dev/null); then
  echo "No agent-wip/$TASK_ID branch (the agent failed before creating it, or resume named an unknown task); nothing to publish." >> "$GITHUB_STEP_SUMMARY"
  exit 0
fi
ahead=$(gh api "repos/$REPO/compare/$BASE...$sha" --jq .ahead_by)
if [ "$ahead" -eq 0 ]; then
  echo "The agent pushed nothing beyond $BASE; no pull request." >> "$GITHUB_STEP_SUMMARY"
  exit 0
fi

# Fast-forward only. A resume continues from agent-wip/<task>, which
# always descends from what was last published; anything else is a
# rewritten history that a reviewer should see rather than have
# silently force-pushed over the PR they are reading.
if gh api "repos/$REPO/git/ref/heads/$head" >/dev/null 2>&1; then
  gh api -X PATCH "repos/$REPO/git/refs/heads/$head" -f sha="$sha" -F force=false >/dev/null
else
  gh api -X POST "repos/$REPO/git/refs" -f ref="refs/heads/$head" -f sha="$sha" >/dev/null
fi

# The agent's structured result is data, read with jq and passed on
# through files and arguments, never evaluated.
field() {
  [ -n "$RESULT" ] || return 0
  printf '%s' "$RESULT" | jq -r --arg k "$1" '.[$k] // empty' 2>/dev/null || true
}
status=$(field status)
[ -n "$status" ] || status=none
title=$(field title)
[ -n "$title" ] || title="[agent] task $TASK_ID (no summary returned)"
title=$(printf '%s' "$title" | head -n1 | cut -c1-120)
draft=true
if [ "$OUTCOME" = success ] && [ "$status" = complete ]; then draft=false; fi

body_file=$(mktemp)
{
  field body
  echo
  echo "---"
  echo "**Agent task \`$TASK_ID\`** · agent step \`$OUTCOME\` · reported status \`$status\` · [run log]($RUN_URL)"
  echo
  echo "Continue it with \`agent-resume $TASK_ID \"…\"\` (scripts/agent.sh)."
  echo
  echo "<details><summary>Task as dispatched</summary>"
  echo
  echo '````text'
  printf '%s\n' "$TASK"
  echo '````'
  echo "</details>"
} > "$body_file"

number=$(gh api "repos/$REPO/pulls?head=${REPO%%/*}:$head&state=open" --jq '.[0].number // empty')
if [ -z "$number" ]; then
  url=$(gh api -X POST "repos/$REPO/pulls" \
    -f title="$title" -f head="$head" -f base="$BASE" \
    -F draft="$draft" -F body=@"$body_file" --jq .html_url)
else
  url=$(gh api -X PATCH "repos/$REPO/pulls/$number" \
    -f title="$title" -F body=@"$body_file" --jq .html_url)
  if [ "$draft" = false ]; then
    # Draft -> ready has no REST endpoint; it is a GraphQL mutation.
    node_id=$(gh api "repos/$REPO/pulls/$number" --jq .node_id)
    gh api graphql -f query='mutation($id:ID!){markPullRequestReadyForReview(input:{pullRequestId:$id}){clientMutationId}}' \
      -f id="$node_id" >/dev/null ||
      echo "::warning::could not mark #$number ready for review; it stays a draft"
  fi
fi
echo "Pull request: $url (draft: $draft)" >> "$GITHUB_STEP_SUMMARY"
