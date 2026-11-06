#!/usr/bin/env bash
# Self-test for workflow-shell-check.sh, the gate that keeps inline shell out
# of workflow `run:` blocks.
#
# WHY A LAYOUT GATE NEEDS A TEST. Its whole job is to notice something, so the
# way it breaks is by noticing nothing — a detector whose block parser stops
# matching reports a clean tree forever, and reads exactly like success. This
# repo has been bitten by that shape repeatedly (the S9 pin gate enforced
# nothing until #467; skills-check truncated and skipped lint). So each rule is
# asserted twice: a fixture it must REJECT, and a fixture it must ACCEPT.
#
# The fixtures are written to a temp dir and the gate is pointed at them with
# WORKFLOW_DIR, so none of this touches .github/workflows.
#
# Run: bash scripts/ci/workflow-shell-check-test.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2

GATE=scripts/ci/workflow-shell-check.sh
pass=0
fail=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# case <name> <want-exit> <fixture-body>
case_() {
	local name=$1 want=$2 body=$3
	local d="$tmp/$RANDOM.$pass.$fail"
	mkdir -p "$d"
	printf '%s\n' "$body" > "$d/wf.yml"
	local got out
	out=$(WORKFLOW_DIR="$d" bash "$GATE" 2>&1)
	got=$?
	if [ "$got" = "$want" ]; then
		pass=$((pass + 1))
		printf 'ok   %s\n' "$name"
	else
		fail=$((fail + 1))
		printf 'FAIL %s\n       want exit: %s\n        got exit: %s\n        output: %s\n' \
			"$name" "$want" "$got" "$out"
	fi
}

# --- REJECTED: over the line budget ---------------------------------------
case_ 'rejects 4 lines of shell' 1 'on: push
jobs:
  a:
    steps:
      - run: |
          echo one
          echo two
          echo three
          echo four'

# --- ACCEPTED: at the budget ----------------------------------------------
case_ 'accepts 3 lines of shell' 0 'on: push
jobs:
  a:
    steps:
      - run: |
          echo one
          echo two
          echo three'

# --- ACCEPTED: comments and blanks are free -------------------------------
# The budget must not discourage the long explanations this repo's steps carry.
case_ 'comments and blanks do not count' 0 'on: push
jobs:
  a:
    steps:
      - run: |
          # a long explanation
          #
          # that goes on for a while
          echo one

          # and more
          echo two'

# --- REJECTED: control flow, under the budget -----------------------------
# The load-bearing clause. Each of these is within any length budget and is
# still a decision nothing can test in place.
case_ 'rejects an if, under budget' 1 'on: push
jobs:
  a:
    steps:
      - run: |
          if [ -n "$X" ]; then echo y; fi'
case_ 'rejects a for loop' 1 'on: push
jobs:
  a:
    steps:
      - run: |
          for f in a b; do echo "$f"; done'
case_ 'rejects a while loop' 1 'on: push
jobs:
  a:
    steps:
      - run: |
          while read -r l; do echo "$l"; done'
case_ 'rejects a case statement' 1 'on: push
jobs:
  a:
    steps:
      - run: |
          case "$X" in a) echo a ;; esac'
case_ 'rejects a function definition' 1 'on: push
jobs:
  a:
    steps:
      - run: |
          helper() {
            echo hi
          }'

# --- ACCEPTED: words that only LOOK like control flow ---------------------
# The anchors matter: an unanchored /if/ would reject `--if-present`, and a
# bare /for/ would reject `--format`.
case_ 'accepts flags that contain keywords' 0 'on: push
jobs:
  a:
    steps:
      - run: |
          pnpm --if-present build
          cargo fmt --format-version 1
          echo caseless whiled'

# --- ACCEPTED: a folded scalar is ONE command, however long ---------------
# `run: >` joins its lines with spaces. Counting its physical lines is the
# wrong question, and an earlier draft of the gate got this wrong against
# rust.yml's pg-suite steps.
#
# NOTE THE BENIGN LITERAL BLOCK in these two fixtures. Without it the file
# holds no literal block at all, positive control #2 fires, and the case exits
# 2 — correctly. Pairing each with a passing literal block is what makes the
# assertion "the folded step is not itself reported" rather than "this file
# happens to contain nothing the parser recognises".
case_ 'accepts a long folded run: >' 0 'on: push
jobs:
  a:
    steps:
      - run: >
          cargo test -p er-server --test main --
          pg_a pg_b pg_c pg_d
          pg_e pg_f pg_g pg_h
          pg_i pg_j pg_k pg_l
      - run: |
          echo one
          echo two'

# --- ACCEPTED: a plain one-line run:, whatever its length -----------------
case_ 'accepts a long single-line run:' 0 'on: push
jobs:
  a:
    steps:
      - run: cargo test --workspace --all-targets --no-fail-fast -- --nocapture --test-threads 4
      - run: |
          echo one
          echo two'

# --- the block must END at the dedent -------------------------------------
# If the parser kept consuming past the block, the keys after it would be
# counted as shell and this would be rejected.
case_ 'the block ends at the dedent' 0 'on: push
jobs:
  a:
    steps:
      - run: |
          echo one
          echo two
        env:
          A: 1
          B: 2
          C: 3
          D: 4
      - run: echo after'

# --- every offending block is reported, not just the first ----------------
multi=$(
	d="$tmp/multi"
	mkdir -p "$d"
	printf '%s\n' 'on: push
jobs:
  a:
    steps:
      - run: |
          for f in a b; do echo "$f"; done
      - run: |
          if [ -n "$X" ]; then echo y; fi' > "$d/wf.yml"
	WORKFLOW_DIR="$d" bash "$GATE" 2>&1 | grep -c 'wf.yml:'
)
if [ "${multi:-0}" -eq 2 ]; then
	pass=$((pass + 1))
	echo 'ok   reports every offending block, not just the first'
else
	fail=$((fail + 1))
	printf 'FAIL reports every offending block\n       want: 2\n        got: %s\n' "$multi"
fi

# --- generated workflows are skipped --------------------------------------
# `gh aw compile` output is not ours to move. A generated file that would
# otherwise be rejected must be skipped — and because skipping everything is
# how this gate would silently stop working, that case must exit 2, not 0.
case_ 'skips a generated workflow (and then has nothing to check)' 2 '# This file was automatically generated. DO NOT EDIT.
on: push
jobs:
  a:
    steps:
      - run: |
          for f in a b; do echo "$f"; done'

# --- POSITIVE CONTROLS ----------------------------------------------------
# Both must exit 2 (a broken gate), never 0 (a clean tree).
mkdir -p "$tmp/empty"
out=$(WORKFLOW_DIR="$tmp/empty" bash "$GATE" 2>&1)
got=$?
if [ "$got" = 2 ]; then
	pass=$((pass + 1))
	echo 'ok   no workflows at all exits 2, not 0'
else
	fail=$((fail + 1))
	printf 'FAIL no workflows at all exits 2\n        got: %s (%s)\n' "$got" "$out"
fi

# A tree with workflows but not one literal block: the parser found nothing,
# which must be reported as broken rather than clean.
case_ 'workflows but zero literal blocks exits 2' 2 'on: push
jobs:
  a:
    steps:
      - run: echo only single-line steps here'

echo "---"
echo "pass=$pass fail=$fail"
if [ "$pass" -eq 0 ]; then
	echo "FAIL asserted nothing"
	exit 2
fi
[ "$fail" -eq 0 ] || exit 1
echo "workflow-shell-check-test: ok"
