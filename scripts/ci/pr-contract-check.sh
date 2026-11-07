#!/usr/bin/env bash
# The `pull request contract` gate: read one pull request, apply
# scripts/ci/pr-contract.sh, annotate, exit non-zero if the contract is unmet.
#
# All of the decision logic is in the sourced file so pr-contract-test.sh can
# exercise it with no network. This wrapper is the only part that talks to
# GitHub, and it does exactly one query.
#
# Inputs (env): PR_NUMBER, GITHUB_REPOSITORY, GH_TOKEN.
# Run: PR_NUMBER=806 GITHUB_REPOSITORY=athvin/entity-resolution-engine \
#        bash scripts/ci/pr-contract-check.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2
# shellcheck source=scripts/ci/pr-contract.sh
. scripts/ci/pr-contract.sh

: "${PR_NUMBER:?PR_NUMBER is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
export PR_REPO="$GITHUB_REPOSITORY"

# `gh pr view --json closingIssuesReferences` omits each reference's
# repository — it came back `null` when probed — so a cross-repository closing
# reference would read as a local one. GraphQL is asked directly for that field.
read -r owner name <<<"${GITHUB_REPOSITORY/\// }"
query='query($owner:String!,$name:String!,$n:Int!){
  repository(owner:$owner,name:$name){ pullRequest(number:$n){
    headRefName title body
    author{login}
    labels(first:50){nodes{name}}
    closingIssuesReferences(first:20){nodes{number repository{nameWithOwner}}}
  }}}'

# A FAILED API CALL IS NOT DATA. Interpolating an unchecked call into the facts
# below would turn a 403 into an empty label list and a missing ticket — the
# gate accusing the pull request of the permissions problem. Fail loudly on the
# call instead, naming the permission, because that is the fix.
if ! facts=$(gh api graphql -f query="$query" \
	-F owner="$owner" -F name="$name" -F n="$PR_NUMBER" \
	--jq '.data.repository.pullRequest' 2>&1); then
	echo "::error::could not read pull request #${PR_NUMBER} — needs \`pull-requests: read\` and \`contents: read\`"
	printf '%s\n' "$facts" | sed 's/^/  /'
	exit 1
fi

PR_HEAD_REF=$(printf '%s' "$facts" | jq -r '.headRefName // ""')
PR_TITLE=$(printf '%s' "$facts" | jq -r '.title // ""')
PR_AUTHOR=$(printf '%s' "$facts" | jq -r '.author.login // ""')
PR_LABELS=$(printf '%s' "$facts" | jq -r '.labels.nodes[]?.name')
PR_CLOSING_ISSUES=$(printf '%s' "$facts" |
	jq -r '.closingIssuesReferences.nodes[]? | "\(.repository.nameWithOwner)#\(.number)"')
export PR_HEAD_REF PR_TITLE PR_AUTHOR PR_LABELS PR_CLOSING_ISSUES

body=$(mktemp)
trap 'rm -f "$body"' EXIT
printf '%s' "$facts" | jq -r '.body // ""' >"$body"

verdict=$(pr_contract_classify <"$body")
ok=$?
field() { printf '%s\n' "$verdict" | sed -n "s/^$1=//p"; }
ticket=$(field ticket)
ledger=$(field ledger)

case "$ticket" in
none) echo "::error::no ticket — this pull request closes no issue in ${GITHUB_REPOSITORY}. Add \`Closes #<n>\` naming its board ticket (file one if it does not exist). A pull request that closes nothing moves no card. Deliberate exception: the \`no-ticket\` label." ;;
foreign) echo "::error::no ticket — every closing reference is in another repository. The board tracks issues in ${GITHUB_REPOSITORY}; close one of those." ;;
unknown-repo) echo "::error::GITHUB_REPOSITORY was empty, so the ticket could not be attributed. This is a workflow misconfiguration, not a problem with the pull request." ;;
esac

case "$ledger" in
missing) echo "::error::no post-merge ledger — restore the \`${PR_LEDGER_BEGIN} … ${PR_LEDGER_END}\` block from .github/pull_request_template.md. Empty boxes are expected here; the ledger is ticked after the merge, and it is the only record that the follow-up work happened." ;;
inverted) echo "::error::the post-merge ledger markers are out of order — \`${PR_LEDGER_END}\` appears before \`${PR_LEDGER_BEGIN}\`." ;;
empty) echo "::error::the post-merge ledger has no checkboxes between its markers — restore them from .github/pull_request_template.md." ;;
esac

if [ "$ok" -eq 0 ]; then
	case "$(field verdict)" in
	exempt) echo "::notice::pull request contract exempt — $(field reason)" ;;
	*) printf 'contract satisfied — closes %s, ledger present\n' "$(printf '%s' "$PR_CLOSING_ISSUES" | tr '\n' ' ')" ;;
	esac
fi

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
	{
		echo "### pull request contract"
		echo
		echo "| Clause | Verdict |"
		echo "|---|---|"
		echo "| ticket (\`Closes #<n>\`) | \`${ticket}\` |"
		echo "| post-merge ledger | \`${ledger}\` |"
	} >>"$GITHUB_STEP_SUMMARY"
fi

exit "$ok"
