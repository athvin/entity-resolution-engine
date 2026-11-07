#!/usr/bin/env bash
# Self-test for pr-contract.sh, the classifier behind the `pull request
# contract` check.
#
# Same reason detect-paths-test.sh exists: a detector breaks by noticing
# nothing, which reads exactly like a clean tree. Both directions are asserted,
# and the load-bearing cases are the EXEMPTIONS — a false failure on a
# merge-bus pull request stops every batch from landing, which is worse than
# the gate not existing.
#
# The other load-bearing case is `the committed template passes`. The gate
# asserts the ledger markers are present in a pull request body, and that body
# comes from .github/pull_request_template.md — so an edit that drops a marker
# would fail every pull request opened afterwards, with nothing in the diff
# that mentions CI.
#
# Run: bash scripts/ci/pr-contract-test.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2
# shellcheck source=scripts/ci/pr-contract.sh
. scripts/ci/pr-contract.sh

pass=0
fail=0

ok() {
	pass=$((pass + 1))
	printf 'ok   %s\n' "$1"
}
bad() {
	fail=$((fail + 1))
	printf 'FAIL %s\n       want: %s\n        got: %s\n' "$1" "$2" "$3"
}

# expect <name> <want-verdict> — body on stdin, facts already exported.
expect() {
	local name=$1 want=$2 out got
	out=$(pr_contract_classify)
	got=$(printf '%s\n' "$out" | sed -n 's/^verdict=//p')
	[ "$got" = "$want" ] && ok "$name" || bad "$name" "$want" "$got"
}

# expect_field <name> <field> <want> — body on stdin.
expect_field() {
	local name=$1 fld=$2 want=$3 out got
	out=$(pr_contract_classify)
	got=$(printf '%s\n' "$out" | sed -n "s/^$fld=//p")
	[ "$got" = "$want" ] && ok "$name" || bad "$name" "$want" "$got"
}

reset_facts() {
	export PR_REPO='athvin/entity-resolution-engine'
	export PR_HEAD_REF='issue/807-pr-contract'
	export PR_AUTHOR='athvin'
	export PR_TITLE='feat(ci): the pull request contract'
	export PR_LABELS=''
	export PR_CLOSING_ISSUES='athvin/entity-resolution-engine#807'
}

LEDGER=$(printf '%s\n- [ ] main is green\n%s\n' "$PR_LEDGER_BEGIN" "$PR_LEDGER_END")

# --- POSITIVE CONTROL ------------------------------------------------------
# Everything below asserts against the real template. If it is missing, every
# such assertion would pass vacuously on an empty string.
TEMPLATE=.github/pull_request_template.md
if [ -s "$TEMPLATE" ]; then
	ok 'the template exists and is not empty'
else
	bad 'the template exists and is not empty' 'a non-empty file' "$TEMPLATE missing"
fi

# --- the committed template satisfies its own gate -------------------------
reset_facts
expect 'the committed template, with a ticket' pass <"$TEMPLATE"

reset_facts
expect_field 'the template ledger scans ok' ledger ok <"$TEMPLATE"

# Every box in a fresh template is open, and no number is hard-coded here: a
# count in a test is a count that goes stale when a ledger item is added.
# Five fields, not four: a short `read` silently folds the trailing ones into
# the last variable, which is how adding `loose` to the scan broke this
# assertion rather than the code it guards.
read -r _bl _el boxes open _loose <<<"$(pr_ledger_scan <"$TEMPLATE")"
if [ "$boxes" -gt 0 ] && [ "$open" -eq "$boxes" ]; then
	ok "a fresh template ledger is entirely unticked ($open/$boxes)"
else
	bad 'a fresh template ledger is entirely unticked' 'open == boxes > 0' "boxes=$boxes open=$open"
fi

# --- the ticket clause -----------------------------------------------------
reset_facts
PR_CLOSING_ISSUES=''
expect_field 'no closing reference at all' ticket none <<<"$LEDGER"

reset_facts
PR_CLOSING_ISSUES='athvin/some-other-repo#12'
expect_field 'a closing reference in another repository' ticket foreign <<<"$LEDGER"

reset_facts
PR_CLOSING_ISSUES=$(printf 'athvin/other#1\nathvin/entity-resolution-engine#807\n')
expect_field 'one local reference among foreign ones' ticket ok <<<"$LEDGER"

reset_facts
PR_REPO=''
GITHUB_REPOSITORY=''
expect_field 'an unset repository is a config error, not a verdict' ticket unknown-repo <<<"$LEDGER"
unset GITHUB_REPOSITORY

# A body that merely MENTIONS a closing keyword proves nothing: the gate reads
# GitHub's parsed references, so prose cannot satisfy it. This is the case a
# regex implementation would get wrong.
reset_facts
PR_CLOSING_ISSUES=''
expect 'prose saying "Closes #807" with no parsed reference' fail <<EOF
Closes #807
$LEDGER
EOF

# --- the ledger clause -----------------------------------------------------
reset_facts
expect_field 'no ledger block' ledger missing <<<'## What

A change.'

reset_facts
expect_field 'markers in the wrong order' ledger inverted <<EOF
$PR_LEDGER_END
- [ ] main is green
$PR_LEDGER_BEGIN
EOF

reset_facts
expect_field 'markers with no checkboxes between them' ledger empty <<EOF
$PR_LEDGER_BEGIN
## Post-merge ledger

Closed by: nobody
$PR_LEDGER_END
EOF

reset_facts
expect_field 'a checkbox OUTSIDE the markers does not count' ledger empty <<EOF
- [ ] something else entirely
$PR_LEDGER_BEGIN
nothing here
$PR_LEDGER_END
- [ ] nor this
EOF

reset_facts
expect 'a ticket and a ledger' pass <<<"$LEDGER"

# --- unticked counting, which the audit reads ------------------------------
closed=$(printf '%s\n- [x] main is green — 2860240d\n- [X] images — n/a\n%s\n' \
	"$PR_LEDGER_BEGIN" "$PR_LEDGER_END")
got=$(pr_ledger_unticked <<<"$closed")
[ "$got" = 0 ] && ok 'a fully ticked ledger counts 0 unticked' ||
	bad 'a fully ticked ledger counts 0 unticked' 0 "$got"

half=$(printf '%s\n- [x] main is green\n- [ ] deploy\n- [ ] follow-ups\n%s\n' \
	"$PR_LEDGER_BEGIN" "$PR_LEDGER_END")
got=$(pr_ledger_unticked <<<"$half")
[ "$got" = 2 ] && ok 'a half-worked ledger counts its open boxes' ||
	bad 'a half-worked ledger counts its open boxes' 2 "$got"

got=$(pr_ledger_unticked <<<'no ledger here')
[ "$got" = 0 ] && ok 'a body with no ledger counts 0 (absence is the gate'"'"'s job)' ||
	bad 'a body with no ledger counts 0' 0 "$got"

# --- deferrals, where the annotation must name a ticket --------------------
# `deferred` TICKS the box, so without the ticket requirement it would be the
# way to retire an item without doing it or tracking it anywhere.
loose() { pr_ledger_loose_deferrals; }

got=$(loose <<EOF
$PR_LEDGER_BEGIN
- [x] **follow-ups** — deferred — #810 owns the sweep
$PR_LEDGER_END
EOF
)
[ "$got" = 0 ] && ok 'a deferral naming a ticket is tracked, not loose' ||
	bad 'a deferral naming a ticket is tracked' 0 "$got"

got=$(loose <<EOF
$PR_LEDGER_BEGIN
- [x] **follow-ups** — deferred, will get to it
$PR_LEDGER_END
EOF
)
[ "$got" = 1 ] && ok 'a deferral with no ticket is LOOSE' ||
	bad 'a deferral with no ticket is LOOSE' 1 "$got"

# The annotation usually lands on a WRAPPED line, which a per-line test would
# judge separately from the tick. This is the case the grouping exists for.
got=$(loose <<EOF
$PR_LEDGER_BEGIN
- [x] **follow-ups** — everything deferred here is owned by
      ticket #810, filed before the merge
$PR_LEDGER_END
EOF
)
[ "$got" = 0 ] && ok 'a ticket on a continuation line still counts' ||
	bad 'a ticket on a continuation line still counts' 0 "$got"

got=$(loose <<EOF
$PR_LEDGER_BEGIN
- [x] **follow-ups** — deferred because the cluster is
      unreachable from this session
$PR_LEDGER_END
EOF
)
[ "$got" = 1 ] && ok 'a wrapped deferral with no ticket anywhere is LOOSE' ||
	bad 'a wrapped deferral with no ticket anywhere is LOOSE' 1 "$got"

# An UNTICKED box saying `blocked` is already counted as open; it must not be
# double-reported as a loose deferral just because the word appears.
got=$(loose <<EOF
$PR_LEDGER_BEGIN
- [ ] **deploy** — blocked, no cluster access; deferred judgement
$PR_LEDGER_END
EOF
)
[ "$got" = 0 ] && ok 'an unticked box is open, never a loose deferral' ||
	bad 'an unticked box is open, never a loose deferral' 0 "$got"

# Grouping must not disturb the counts the audit already relied on.
mixed=$(printf '%s\n- [x] one — n/a\n- [ ] two\n- [x] three — deferred — #1\n- [ ] four\n%s\n' \
	"$PR_LEDGER_BEGIN" "$PR_LEDGER_END")
read -r _b _e boxes open loose5 <<<"$(pr_ledger_scan <<<"$mixed")"
if [ "$boxes" = 4 ] && [ "$open" = 2 ] && [ "$loose5" = 0 ]; then
	ok 'grouping preserves boxes/unticked while adding the fifth field'
else
	bad 'grouping preserves boxes/unticked' 'boxes=4 open=2 loose=0' "boxes=$boxes open=$open loose=$loose5"
fi

# The committed template must not ship a loose deferral of its own.
got=$(loose <"$TEMPLATE")
[ "$got" = 0 ] && ok 'the committed template has no loose deferral' ||
	bad 'the committed template has no loose deferral' 0 "$got"

# --- the exemptions, where a false FAILURE is the expensive direction ------
# Each is asserted with NEITHER clause satisfied, because that is the state the
# exempt pull requests are actually in.
reset_facts
PR_CLOSING_ISSUES=''
PR_HEAD_REF='merge-bus/19284756'
expect 'a merge-bus pull request' exempt <<<'### riding: #1, #2'

reset_facts
PR_CLOSING_ISSUES=''
PR_AUTHOR='dependabot[bot]'
PR_TITLE='chore(deps): bump serde from 1.0.1 to 1.0.2'
expect 'a dependabot bump' exempt <<<'Bumps serde.'

reset_facts
PR_CLOSING_ISSUES=''
PR_TITLE='Revert "feat(ci): the pull request contract"'
expect 'a revert' exempt <<<'This reverts commit abc1234.'

reset_facts
PR_CLOSING_ISSUES=''
PR_LABELS=$(printf 'documentation\nno-ticket\n')
expect 'the no-ticket override label' exempt <<<'A one-word typo fix.'

# A label that merely CONTAINS the override name must not exempt: the match is
# whole-line, so `no-ticket-yet` is a different label.
reset_facts
PR_CLOSING_ISSUES=''
PR_LABELS='no-ticket-yet'
expect 'a label that only resembles the override' fail <<<"$LEDGER"

# --- report ----------------------------------------------------------------
printf '\npr-contract-test: %d passed, %d failed\n' "$pass" "$fail"
# A run that asserted nothing is a broken test, not a clean one.
if [ "$pass" -eq 0 ]; then
	echo 'pr-contract-test: asserted NOTHING — the harness is broken' >&2
	exit 2
fi
[ "$fail" -eq 0 ]
