#!/usr/bin/env bash
# Run every merge-bus scenario and assert on what the bus actually did.
#
#   bash scripts/ci/merge-bus-test.sh
#
# `merge-bus.yml` is ~540 lines of bash that merges to `main`, and
# `workflow_dispatch` only fires for a workflow already on the default branch —
# so the live path cannot be exercised before the thing is merged. This runs the
# workflow's REAL step scripts, with a runner's semantics for the parts that
# carry bugs (per-step `$GITHUB_OUTPUT`, `${{ }}` in `env:`, `if:` gating, a
# failed step halting the job), against a local bare repo with a stubbed `gh`.
# Git operations are genuine; only the GitHub API is faked.
#
# The scenarios, covering the green path and every failure path:
#   green stacked red starved preflight moved partial reject collide lying
#
# Each was mutation-tested — the corresponding guard was deliberately removed
# from the workflow and the suite was confirmed to go red. `lying` exists
# BECAUSE of that exercise: without it, deleting the land step's tree assertion
# left every scenario still passing, since the tree always matched. It now
# simulates `gh pr merge` reporting success while landing nothing.
#
# HAND-RUN, like the skills' `evals.md` scenarios — there is no CI job for it
# yet, deliberately: it needs python3 + PyYAML, and a required check may not be
# added before it has been observed failing. Run it when you touch the bus.
#
# Exit codes: 0 every assertion held; 1 at least one did not.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)/merge-bus-sim"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export WF="${WF:-$REPO_ROOT/.github/workflows/merge-bus.yml}"
[ -f "$WF" ] || { echo "merge-bus-test: no workflow at $WF" >&2; exit 2; }
python3 -c 'import yaml' 2>/dev/null || { echo "merge-bus-test: needs python3 with PyYAML" >&2; exit 2; }

pass=0; fail=0
ok()   { printf '    ok   %s\n' "$1"; pass=$((pass+1)); }
bad()  { printf '    FAIL %s\n' "$1"; fail=$((fail+1)); }
chk()  { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (want '$3', got '$2')"; fi; }
has()  { if grep -qF -- "$2" "$1"; then ok "$3"; else bad "$3"; fi; }
hasnt(){ if grep -qF -- "$2" "$1"; then bad "$3"; else ok "$3"; fi; }

run() { SIM="$(bash "$HERE/scenario.sh" "$1" 2>/dev/null | tail -1)"; echo "$SIM"; }
mainfiles() { git -C "$1/origin.git" ls-tree -r --name-only refs/heads/main | tr '\n' ' '; }

echo "=== green: two clean members land, in order ==="
S=$(run green); echo "  rc=$(cat "$S/rc")"
chk "exit 0" "$(cat "$S/rc")" "0"
has "$S/log" "landed #101" "#101 landed"
has "$S/log" "landed #102" "#102 landed"
has "$S/log" "main's tree matches the validated batch tree" "tree assertion passed"
chk "main has both files" "$(mainfiles "$S")" "src/a.txt src/b.txt src/base.txt "
has "$S/calls.log" "pr close 900" "bus PR closed"

echo "=== stacked: a child of a member is retargeted BEFORE the parent merges ==="
S=$(run stacked); echo "  rc=$(cat "$S/rc")"
chk "exit 0" "$(cat "$S/rc")" "0"
has "$S/log" "retargeting #201" "child retargeted"
# Order matters: the edit must precede the merge, or GitHub marks it merged.
if [ "$(grep -n 'pr edit 201 --base main' "$S/calls.log" | cut -d: -f1 | head -1)" -lt \
     "$(grep -n 'pr merge 101' "$S/calls.log" | cut -d: -f1 | head -1)" ]; then
  ok "retarget happened BEFORE the parent merge"
else
  bad "retarget happened before the parent merge"
fi

echo "=== red: nothing merges, members told, bus labelled ==="
S=$(run red); echo "  rc=$(cat "$S/rc")"
chk "exit non-zero" "$([ "$(cat "$S/rc")" != "0" ] && echo yes || echo no)" "yes"
has   "$S/log" "went **red** on: test, entities" "failing contexts named"
hasnt "$S/calls.log" "pr merge" "nothing was merged"
has   "$S/calls.log" "pr edit 900 --add-label bus-failed" "bus-failed applied"
has   "$S/calls.log" "pr comment 101" "member #101 told"
chk "main untouched" "$(mainfiles "$S")" "src/base.txt "

echo "=== starved: reported as infrastructure, NOT as a bad batch ==="
S=$(run starved); echo "  rc=$(cat "$S/rc")"
chk "exit non-zero" "$([ "$(cat "$S/rc")" != "0" ] && echo yes || echo no)" "yes"
has   "$S/log" "never ran" "worded as never-ran"
hasnt "$S/calls.log" "add-label bus-failed" "NO bus-failed label"
hasnt "$S/calls.log" "pr comment 101" "members NOT blamed"
hasnt "$S/calls.log" "pr merge" "nothing merged"

echo "=== preflight: refuses to build anything when runners are starved ==="
S=$(run preflight); echo "  rc=$(cat "$S/rc")"
chk "exit non-zero" "$([ "$(cat "$S/rc")" != "0" ] && echo yes || echo no)" "yes"
has   "$S/log" "not allocating runners" "diagnosed as account-level"
hasnt "$S/calls.log" "pr create" "no bus PR created"
hasnt "$S/log" "merge-bus: #" "no batch built"

echo "=== moved: main moves mid-validation, batch is abandoned ==="
S=$(run moved); echo "  rc=$(cat "$S/rc")"
chk "exit non-zero" "$([ "$(cat "$S/rc")" != "0" ] && echo yes || echo no)" "yes"
has   "$S/log" "main moved from" "abort names the move"
hasnt "$S/calls.log" "pr merge" "nothing merged after the move"

echo "=== partial: a mid-sequence merge failure is reported honestly ==="
S=$(run partial); echo "  rc=$(cat "$S/rc")"
chk "exit non-zero" "$([ "$(cat "$S/rc")" != "0" ] && echo yes || echo no)" "yes"
has "$S/log" "landed #101" "#101 did land"
has "$S/log" "Landed: 101" "reports which landed"
chk "main has only a.txt" "$(mainfiles "$S")" "src/a.txt src/base.txt "

echo "=== reject: DIRTY, stacked and draft candidates are all excluded ==="
S=$(run reject); echo "  rc=$(cat "$S/rc")"
has "$S/log" "conflicts with main" "DIRTY rejected by name"
has "$S/log" "stacked on 'feat-a', not main" "stacked rejected"
has "$S/log" "draft" "draft rejected"
has "$S/log" "batch: 101 102" "only the clean two ride"

echo "=== collide: a conflicting member is ejected, the batch continues ==="
S=$(run collide); echo "  rc=$(cat "$S/rc")"
chk "exit 0" "$(cat "$S/rc")" "0"
has "$S/log" "#107 conflicts with the batch and was ejected" "conflictor ejected"
has "$S/calls.log" "pr edit 107 --add-label bus-conflict" "bus-conflict applied"
has "$S/log" "landed #102" "#102 still landed after the ejection"
hasnt "$S/calls.log" "pr merge 107" "the ejected PR was never merged"
has "$S/log" "main's tree matches the validated batch tree" "tree assertion still holds"

echo "=== lying merge: GitHub reports merged but nothing landed ==="
S=$(run lying); echo "  rc=$(cat "$S/rc")"
chk "exit non-zero" "$([ "$(cat "$S/rc")" != "0" ] && echo yes || echo no)" "yes"
has "$S/log" "but the gate validated" "tree assertion FIRED on the divergence"
has "$S/log" "Something did not land as tested" "says what it means"
chk "main is missing b.txt" "$(mainfiles "$S")" "src/a.txt src/base.txt "

echo
echo "===================================="
echo "passed: $pass   failed: $fail"
[ "$fail" -eq 0 ] || exit 1
