#!/usr/bin/env bash
# infra.yml's comment job: assemble the plan comment and upsert it by MARKER.
# `tf-` prefix on purpose: a genuine terraform helper (see tf-plan-run.sh).
#
# Upserted by MARKER, not `gh pr comment --edit-last` (that keys on the
# authoring actor). The payload goes through `jq -Rs` so no part of a plan's
# text can be read as a field type or a flag. THE LOOKUP IS TWO STATEMENTS,
# NOT A PIPELINE: under `bash -e` (no pipefail) a failing `gh` piped into jq
# is invisible — jq succeeds on empty input and the job posts a duplicate
# comment on every push. Assigning `gh`'s output first makes its failure
# fatal; `--slurp` collects every page so the filter emits exactly one line.
#
# Inputs (env): GH_TOKEN, PR, MARKER, GITHUB_REPOSITORY. Reads tfplans/.
if ! bash scripts/ci/tf-plan-comment.sh tfplans > body.md; then
  echo "no plan artifacts to report — leaving any existing comment alone"
  exit 0
fi
echo "comment body: $(wc -c < body.md) bytes"
jq -Rs '{body: .}' < body.md > payload.json

comments="$(gh api --paginate --slurp "repos/$GITHUB_REPOSITORY/issues/$PR/comments")"
id="$(printf '%s' "$comments" | jq -r --arg m "$MARKER" \
        '[.[][] | select(.body | contains($m))] | .[0].id // empty')"
if [ -n "$id" ]; then
  gh api -X PATCH "repos/$GITHUB_REPOSITORY/issues/comments/$id" \
    --input payload.json > /dev/null
  echo "updated comment $id"
else
  gh api -X POST "repos/$GITHUB_REPOSITORY/issues/$PR/comments" \
    --input payload.json > /dev/null
  echo "created the plan comment"
fi
