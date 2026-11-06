#!/usr/bin/env bash
# Step 2 of merge-bus.yml: build the combination branch and open the bus PR.
#
# Inputs (env): MEMBERS, BRANCH, GITHUB_REPOSITORY, GITHUB_RUN_ID, GH_TOKEN.
# Outputs ($GITHUB_OUTPUT): bus, bus_sha, base_sha, batch_tree, riding, branch.
set -euo pipefail

# PRE-FLIGHT: IS GITHUB ALLOCATING RUNNERS AT ALL? When the account
# hits its Actions minutes or spending limit, every job fails within
# seconds having executed NOTHING — `steps: []`, and the log is a
# `BlobNotFound`. GitHub Status reads "operational" throughout, so it
# looks exactly like a broken diff. Observed on this account
# 2026-11-03, repository-wide and sustained for hours.
#
# A bus PR opened in that state could only ever fail, and would then
# accuse the batch of a failure the batch did not cause. So check
# before creating anything, and read it from EXECUTION rather than
# from conclusions: a starved run and a legitimately failing one are
# both `failure`, but only one of them ran a step. Measured on this
# repo: starved runs summed to 0 steps, real ones to 5, 5 and 202.
#
# Several runs rather than one, because a single starved run may just
# be a run that lost a race for capacity — only an unbroken streak is
# evidence of the account-level block.
# A FAILED API CALL IS NOT DATA. The first version interpolated the
# call straight into `for rid in $(gh api ...)`, so when the call
# 403'd its error text became the loop's word list and the next
# request went to `/actions/runs/gh:/jobs` — a 404 on a garbage id.
# Read the status explicitly instead, and treat an unreadable API as
# unknown rather than as an answer.
recent=""
if ! recent="$(gh api "repos/${GITHUB_REPOSITORY}/actions/runs?status=completed&per_page=5" \
                 --jq '.workflow_runs[].id' 2>/dev/null)"; then
  # Advisory, not a gate: this check only saves a wasted batch during
  # an outage, and the verdict classification below is the real
  # safety net. Failing closed here would let a permissions hiccup
  # block every merge.
  echo "::warning::could not read recent run history (needs \`actions: read\`); skipping the capacity pre-flight"
  recent=""
fi

starved=0; checked=0
for rid in $recent; do
  case "$rid" in ''|*[!0-9]*) continue ;; esac
  steps="$(gh api "repos/${GITHUB_REPOSITORY}/actions/runs/${rid}/jobs?per_page=100" \
             --jq '[.jobs[].steps | length] | add // 0' 2>/dev/null || echo skip)"
  case "$steps" in ''|*[!0-9]*) continue ;; esac
  checked=$((checked + 1))
  [ "$steps" -eq 0 ] && starved=$((starved + 1))
done

if [ "$checked" -gt 0 ] && [ "$starved" -eq "$checked" ]; then
  echo "::error::None of the last ${checked} completed runs executed a single step. GitHub is not allocating runners for this account — an Actions minutes or spending limit, not a problem with any PR. A bus PR could only fail for that reason, so nothing was created."
  exit 1
fi
if [ "$checked" -eq 0 ]; then
  echo "pre-flight: no readable run history; proceeding (the verdict step still classifies starvation)"
else
  echo "pre-flight: runners are being allocated (${starved}/${checked} recent runs starved)"
fi

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

git fetch --no-tags origin main
base_sha="$(git rev-parse origin/main)"
echo "base: $base_sha"
git switch --detach "$base_sha"

riding=(); ejected=()
for n in $MEMBERS; do
  # `refs/pull/<n>/head` rather than the branch name: it is immune to a
  # branch rename between selection and now, and it is what the PR
  # actually proposes.
  git fetch --no-tags origin "refs/pull/$n/head:refs/remotes/pr/$n"
  title="$(gh pr view "$n" --json title --jq .title)"
  if git merge --no-ff -m "merge-bus: #$n $title" "refs/remotes/pr/$n"; then
    riding+=("$n")
  else
    # EJECTING A CONFLICT IS NOT THE SAME DECISION AS STOPPING ON RED.
    # A conflict is found before any CI is spent and it names the
    # culprit exactly, so there is nothing to attribute and nothing to
    # gain by abandoning the other members. Report-and-stop governs CI
    # failure, where blame genuinely is ambiguous.
    git merge --abort || true
    ejected+=("$n")
    echo "::warning::#$n conflicts with the batch and was ejected"
    gh pr comment "$n" --body "Ejected from merge-bus batch \`${BRANCH}\` — this branch conflicts with the other PRs in the batch. Rebase on \`main\` and re-add the \`merge-bus\` label." || true
    gh pr edit "$n" --add-label bus-conflict || echo "::warning::could not add the bus-conflict label to #$n"
  fi
done

if [ "${#riding[@]}" -eq 0 ]; then
  echo "::error::every candidate conflicted with the batch; nothing to validate"
  exit 1
fi

batch_tree="$(git rev-parse "HEAD^{tree}")"

# NOT `-q`. A quiet push suppresses exactly the line that says it did
# not go, and the remote ref is read back below because a push's own
# exit code is not proof the remote moved.
git push origin "HEAD:refs/heads/${BRANCH}"
remote_sha="$(git ls-remote origin "refs/heads/${BRANCH}" | cut -f1)"
local_sha="$(git rev-parse HEAD)"
if [ "$remote_sha" != "$local_sha" ]; then
  echo "::error::push did not land — ${BRANCH} is '$remote_sha' on the remote, '$local_sha' locally"
  exit 1
fi

refs=""; for n in "${riding[@]}"; do refs="${refs} #${n}"; done
{
  echo "Batch of ${#riding[@]}:${refs}"
  echo ""
  echo "This PR exists so the ordinary gate runs against the **combination** of those"
  echo "PRs rather than against each one separately against a base that has since moved."
  echo "If it goes green the members are squash-merged in order; if it goes red nothing"
  echo "merges. Opened by \`merge-bus.yml\`, run \`${GITHUB_RUN_ID}\`."
  if [ "${#ejected[@]}" -gt 0 ]; then
    echo ""
    echo "Ejected for conflicting with the batch: ${ejected[*]}"
  fi
} >/tmp/bus-body.md

# NEVER PREDICT A PR NUMBER. `gh pr create` prints the URL of the PR it
# made; the number is read back from that and from nowhere else.
# Guessing one here once merged an unrelated PR under `--admin`.
url="$(gh pr create --base main --head "${BRANCH}" \
         --title "merge bus:${refs}" --body-file /tmp/bus-body.md)"
echo "created: $url"
bus="${url##*/}"
case "$bus" in
  ''|*[!0-9]*) echo "::error::could not read a PR number out of '$url'"; exit 1 ;;
esac

{
  echo "bus=$bus"
  echo "bus_sha=$local_sha"
  echo "base_sha=$base_sha"
  echo "batch_tree=$batch_tree"
  echo "riding=${riding[*]}"
  echo "branch=${BRANCH}"
} >>"$GITHUB_OUTPUT"
