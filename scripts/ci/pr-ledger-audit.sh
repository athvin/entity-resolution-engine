#!/usr/bin/env bash
# Which merged pull requests still have post-merge work owed?
#
# The `pull request contract` check proves a ledger EXISTS before the merge.
# This reads the other end: a merged pull request whose ledger still has empty
# boxes is follow-up work that was promised and not recorded as done. It is the
# reason the ledger is in the body rather than in a transcript — it can be
# swept.
#
# Hand-run, and deliberately NOT a CI gate: nothing a merged pull request
# reports can block anything, and a scheduled job that files no card is noise.
# `/land-pr` closes the ledger of the pull request it lands; this is how you
# find the ones an interrupted session left open.
#
# Inputs (env): LIMIT (default 30) — how many merged pull requests to read,
# newest first. Exits 1 if any has an open ledger, so it can be wired later.
#
# Run: bash scripts/ci/pr-ledger-audit.sh
#      LIMIT=100 bash scripts/ci/pr-ledger-audit.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2
# shellcheck source=scripts/ci/pr-contract.sh
. scripts/ci/pr-contract.sh

LIMIT=${LIMIT:-30}

# `--limit` silently truncates, and a truncated list reads as a clean sweep —
# so the limit is printed with the result rather than left implicit.
if ! prs=$(gh pr list --state merged --limit "$LIMIT" \
	--json number,title,mergedAt \
	--jq '.[] | "\(.number)\t\(.mergedAt)\t\(.title)"'); then
	echo "could not list merged pull requests" >&2
	exit 2
fi

printf 'PR\tMERGED\tLEDGER\tTITLE\n'
open_total=0
seen=0
while IFS=$'\t' read -r n merged title; do
	[ -n "$n" ] || continue
	seen=$((seen + 1))
	body=$(gh pr view "$n" --json body --jq '.body // ""' 2>/dev/null)
	# Distinguish "no ledger" from "a closed ledger": both count zero open
	# boxes, and they mean opposite things. A pull request merged before the
	# contract existed, or an exempt one, has no ledger and owes nothing.
	status=$(printf '%s\n' "$body" | pr_ledger_status)
	case "$status" in
	ok)
		owed=$(printf '%s\n' "$body" | pr_ledger_unticked)
		if [ "$owed" -gt 0 ]; then
			open_total=$((open_total + 1))
			printf '#%s\t%s\tOPEN (%s)\t%s\n' "$n" "${merged%T*}" "$owed" "$title"
		else
			printf '#%s\t%s\tclosed\t%s\n' "$n" "${merged%T*}" "$title"
		fi
		;;
	*) printf '#%s\t%s\t—\t%s\n' "$n" "${merged%T*}" "$title" ;;
	esac
done <<<"$prs"

printf '\n%d merged pull requests read (LIMIT=%s), %d with an open ledger\n' \
	"$seen" "$LIMIT" "$open_total"
[ "$open_total" -eq 0 ]
