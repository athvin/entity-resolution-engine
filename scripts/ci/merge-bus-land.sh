#!/usr/bin/env bash
# Step 4b of merge-bus.yml: green — land the batch.
#
# Inputs (env): BUS, RIDING, BASE_SHA, BATCH_TREE, BRANCH, GH_TOKEN.
set -euo pipefail

# THE INVARIANT THE WHOLE DESIGN RESTS ON. The green just earned
# describes `main@BASE_SHA` plus these members and nothing else. If
# `main` moved while the gate ran, that green no longer describes what
# landing would produce, and the bus is back to merging a stale pass —
# the very thing it exists to prevent.
git fetch --no-tags origin main
now="$(git rev-parse origin/main)"
if [ "$now" != "$BASE_SHA" ]; then
  echo "::error::main moved from ${BASE_SHA} to ${now} while the batch validated; abandoning"
  gh pr comment "$BUS" --body "Abandoned: \`main\` moved from \`${BASE_SHA}\` to \`${now}\` while this batch was validating, so its green no longer describes what merging would produce. Nothing was merged. Re-run the bus." || true
  exit 1
fi

# RETARGET STACKED CHILDREN BEFORE THE PARENT MERGES. Deleting a merged
# PR's branch makes GitHub resolve any PR based on it by marking it
# MERGED — with a real `mergeCommit` oid — rather than retargeting it.
# Observed 2026-11-03 on #438: marked merged, code absent from `main`,
# and it could not be reopened, so the only recovery was a new PR.
# `gh pr edit --base` fails afterwards ("Cannot change the base branch
# of a closed pull request"), so before is the only time this works.
for n in $RIDING; do
  head_ref="$(gh pr view "$n" --json headRefName --jq .headRefName)"
  while IFS= read -r child; do
    [ -n "$child" ] || continue
    echo "retargeting #${child} from '${head_ref}' onto main before #${n} merges"
    gh pr edit "$child" --base main
    gh pr comment "$child" --body "Retargeted onto \`main\`: its base branch \`${head_ref}\` (#${n}) is being merged by the merge bus, and deleting that branch would otherwise mark this PR merged without its code reaching \`main\`. Rebase with \`git rebase --onto origin/main <old-base-sha> <branch>\`." || true
  done < <(gh pr list --state open --base "$head_ref" --json number --jq '.[].number')
done

landed=()
for n in $RIDING; do
  if gh pr merge "$n" --squash --delete-branch; then
    landed+=("$n")
    echo "landed #$n"
  else
    # A PARTIAL LANDING IS POSSIBLE AND MUST NOT READ AS A CLEAN RUN.
    echo "::error::#${n} failed to merge. Landed: ${landed[*]:-none}. NOT landed: this and everything after it."
    gh pr comment "$BUS" --body "Partial landing: ${landed[*]:-none} merged, then #${n} failed. The remaining members did not merge." || true
    exit 1
  fi
done

# PROOF BY CONTENT, NOT BY STATUS. `state: MERGED` is not evidence a
# change reached `main` — a squash makes the member's head unreachable,
# and a stacked PR can report MERGED with a mergeCommit while its code
# is absent entirely. The batch branch's tree is what the gate actually
# examined, so comparing `main`'s tree against it is the only check
# that proves the validated content is what landed.
git fetch --no-tags origin main
main_tree="$(git rev-parse "origin/main^{tree}")"
if [ "$main_tree" = "$BATCH_TREE" ]; then
  echo "main's tree matches the validated batch tree (${BATCH_TREE})"
else
  echo "::error::main's tree is ${main_tree} but the gate validated ${BATCH_TREE}. Something did not land as tested — inspect before merging anything else."
  exit 1
fi

gh pr close "$BUS" --comment "Landed ${landed[*]} by squash, in order. \`main\`'s tree matches the tree this batch validated." --delete-branch
echo "bus done: ${landed[*]}"
