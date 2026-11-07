# The two things every pull request here must carry. Sourced, not executed.
#
# 1. A TICKET. The board is the plan of record, and the `Pull request merged`
#    automation acts on the *linked issue* — a pull request that closes nothing
#    moves no card, so work lands and the board silently does not reflect it.
# 2. A POST-MERGE LEDGER. `/land-pr` already knows the merge is not the finish
#    line; what it lacked was somewhere durable to record which follow-ups
#    happened. A merged pull request's body stays writable, so the body is that
#    place.
#
# WHY THE TICKET CHECK DOES NOT REGEX THE BODY. `closingIssuesReferences` is
# GitHub's own parse of the closing keywords — the same one that actually closes
# the issue on merge. A regex would happily pass `Closes #<n>` inside a fenced
# block, a comment, or with a keyword GitHub does not honour ("Addresses"), and
# the card would then not move for the one reason the gate exists to prevent.
# Checking the parsed references means the gate cannot be satisfied by text that
# does not work.
#
# WHY THE LEDGER CHECK IS ONLY MARKERS AND A COUNT. It asserts the ledger is
# PRESENT, never that it is finished: at pull-request time every box is
# legitimately empty, because the work it records has not happened yet. Tying
# the gate to any more of the template's wording would make a prose edit a
# merge blocker.
#
# THE CLASSIFIER IS PURE so pr-contract-test.sh can feed it fixtures: facts in
# through the environment, body on stdin, a verdict on stdout, no network. The
# wrapper (pr-contract-check.sh) is the only part that talks to GitHub.
#
# Inputs (env), all optional unless noted:
#   PR_REPO            `owner/repo` the pull request lives in (or GITHUB_REPOSITORY)
#   PR_HEAD_REF        head branch name
#   PR_AUTHOR          author login
#   PR_TITLE           pull request title
#   PR_LABELS          label names, one per line
#   PR_CLOSING_ISSUES  `owner/repo#n` per line, from closingIssuesReferences
# Input (stdin): the pull request body.

PR_LEDGER_BEGIN='<!-- BEGIN POST-MERGE -->'
PR_LEDGER_END='<!-- END POST-MERGE -->'

# pr_contract_exemption — print why this pull request is exempt; 0 if it is.
#
# THE MERGE-BUS ENTRY IS LOAD-BEARING, not a convenience. The bus opens an
# ordinary pull request to prove a COMBINATION green, closes no issue, and
# carries a generated body; `merge-bus-verdict.sh` then reads that pull
# request's `statusCheckRollup` and treats any failed context as a red batch.
# A contract failure there would stop every batch from landing — the gate
# blocking the mechanism that exists to keep `main` green.
pr_contract_exemption() {
	case "${PR_HEAD_REF:-}" in
	merge-bus/*)
		echo 'merge-bus branch — a bus pull request closes no issue by design'
		return 0
		;;
	esac
	case "${PR_AUTHOR:-}" in
	dependabot | 'dependabot[bot]' | app/dependabot)
		echo 'dependabot — a dependency bump has no ticket'
		return 0
		;;
	esac
	# A revert is the availability path: `/land-pr` reverts when `main` is red
	# and one diagnosis pass has not produced a fix. Making it wait for a ticket
	# would put bookkeeping in front of a red `main`.
	case "${PR_TITLE:-}" in
	Revert*|revert*)
		echo 'a revert — availability beats bookkeeping'
		return 0
		;;
	esac
	# `grep -c`, never `grep -q`: a matching `grep -q` closes the pipe and
	# SIGPIPEs the writer, so under `pipefail` a FOUND label can report as
	# missing, intermittently. See scripts/ci/scripts-home-check.sh.
	local labelled
	labelled=$(printf '%s\n' "${PR_LABELS:-}" | grep -cx 'no-ticket' || true)
	if [ "${labelled:-0}" -gt 0 ]; then
		echo 'the `no-ticket` label — a deliberate human override'
		return 0
	fi
	return 1
}

# pr_ledger_scan — body on stdin, prints `<begin> <end> <boxes> <unticked>`.
#
# Line numbers of the first marker of each kind (0 if absent), the number of
# checkboxes between them, and how many of those are still empty. One awk pass
# with NO early `exit`: an awk that stops reading SIGPIPEs the writer feeding
# it, which is the same intermittent-false-negative trap as `grep -q`.
pr_ledger_scan() {
	awk -v b="$PR_LEDGER_BEGIN" -v e="$PR_LEDGER_END" '
		index($0, b) && !bl { bl = NR; inblock = 1; next }
		# The end marker is recorded wherever it appears, INCLUDING before the
		# begin marker. Requiring `bl` first made `inverted` unreachable —
		# scrambled markers reported as a ledger that was simply missing, and
		# the self-test caught it.
		index($0, e) && !el { el = NR; inblock = 0; next }
		inblock && /^[[:space:]]*[-*] \[[ xX]\]/ { boxes++ }
		inblock && /^[[:space:]]*[-*] \[ \]/ { open++ }
		END { printf "%d %d %d %d\n", bl+0, el+0, boxes+0, open+0 }
	'
}

# pr_ledger_status — body on stdin, prints ok | missing | inverted | empty.
pr_ledger_status() {
	local bl el boxes open
	read -r bl el boxes open <<<"$(pr_ledger_scan)"
	if [ "$bl" -eq 0 ] || [ "$el" -eq 0 ]; then
		echo missing
	elif [ "$bl" -ge "$el" ]; then
		echo inverted
	elif [ "$boxes" -eq 0 ]; then
		echo empty
	else
		echo ok
	fi
}

# pr_ledger_unticked — body on stdin, prints the count of empty boxes.
# A closed ledger is 0. Used by pr-ledger-audit.sh over merged pull requests;
# never by the gate, which runs before the work it records.
pr_ledger_unticked() {
	local bl el boxes open
	read -r bl el boxes open <<<"$(pr_ledger_scan)"
	echo "$open"
}

# pr_contract_ticket_status — prints ok | none | foreign | unknown-repo.
#
# Matched as a string prefix rather than a regex: a repository name may contain
# `.`, which a regex would read as "any character".
pr_contract_ticket_status() {
	local repo mine
	repo=${PR_REPO:-${GITHUB_REPOSITORY:-}}
	if [ -z "$repo" ]; then
		echo unknown-repo
		return 0
	fi
	if [ -z "${PR_CLOSING_ISSUES:-}" ]; then
		echo none
		return 0
	fi
	mine=$(printf '%s\n' "$PR_CLOSING_ISSUES" |
		awk -v r="$repo" 'index($0, r "#") == 1 { n++ } END { print n+0 }')
	if [ "$mine" -gt 0 ]; then echo ok; else echo foreign; fi
}

# pr_contract_classify — the whole decision. Body on stdin, facts in env.
#
# Prints `ticket=`, `ledger=`, `verdict=` and, when exempt, `reason=`.
# Returns 0 when the contract is satisfied (a pass or an exemption), 1 when not.
pr_contract_classify() {
	local body exempt ticket ledger
	body=$(cat)

	if exempt=$(pr_contract_exemption); then
		printf 'ticket=exempt\nledger=exempt\nverdict=exempt\nreason=%s\n' "$exempt"
		return 0
	fi

	ticket=$(pr_contract_ticket_status)
	ledger=$(printf '%s\n' "$body" | pr_ledger_status)
	printf 'ticket=%s\nledger=%s\n' "$ticket" "$ledger"

	if [ "$ticket" = ok ] && [ "$ledger" = ok ]; then
		echo 'verdict=pass'
		return 0
	fi
	echo 'verdict=fail'
	return 1
}
