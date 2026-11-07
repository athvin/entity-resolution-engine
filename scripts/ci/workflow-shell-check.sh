#!/usr/bin/env bash
# Shell logic lives in scripts/, including the shell that has no file.
#
# THE HALF scripts-home-check.sh CANNOT SEE. That gate asks "is every tracked
# shell FILE under scripts/?" and the answer has been yes for a while. But a
# workflow's `run: |` block is shell with no file, so it is invisible to a
# file-level rule — and that is where it kept accumulating: seven copies of
# the build-cost report, four of the disk reclaim, three helm installs, and a
# 56-line `detect` step whose classification could not be unit-tested because
# it was YAML. Extracting all of that is only half a fix; without this gate
# the next one lands the same way.
#
# THE RULE IS A BUDGET, NOT A BAN, and the line is drawn at a specific claim:
# a couple of commands in a row is a STEP, and reads better where it is used;
# anything with a branch or a loop in it is LOGIC, and logic belongs somewhere
# `bash -n`, a test and a laptop can reach it. So:
#
#   - more than MAX_LINES lines of shell  -> move it to scripts/ci/
#   - ANY control flow, at ANY length     -> move it to scripts/ci/
#
# The second clause is the load-bearing one. A three-line `if` is under any
# length budget and is still a decision nothing can test in place.
#
# Comments and blank lines do not count toward the budget. That is deliberate:
# this repo's steps carry long explanations, and a rule that counted them
# would push back against writing them down.
#
# Generated workflows are skipped — `gh aw compile` emits them and its shell
# is not ours to move. The marker test is the SAME one scripts/ci/actionlint.sh
# uses (`generated|DO NOT EDIT|gh aw` in the first five lines), on purpose: two
# different answers to "is this file generated?" is how one of them goes
# stale.
#
# Run: bash scripts/ci/workflow-shell-check.sh
#
# WORKFLOW_DIR and MAX_LINES exist for scripts/ci/workflow-shell-check-test.sh,
# which points the gate at fixture workflows so it can be observed FAILING on
# each rule before it is trusted on the real tree — `.claude/skills/frontend-testing/references/frontend-testing.md`'s
# rule, applied to a shell gate. No call site sets either.
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2

# Steps at or under this many lines of real shell, with no control flow, may
# stay inline. Raising it needs an argument, not a commit.
MAX_LINES=${MAX_LINES:-3}
WORKFLOW_DIR=${WORKFLOW_DIR:-.github/workflows}

shopt -s nullglob
workflows=("$WORKFLOW_DIR"/*.yml "$WORKFLOW_DIR"/*.yaml)

# POSITIVE CONTROL #1: no workflows at all means the wrong directory, not a
# clean tree — the same zero-matches-is-failure rule actionlint.sh and
# scripts-home-check.sh apply.
if [ ${#workflows[@]} -eq 0 ]; then
	echo "workflow-shell-check: found no workflows under $WORKFLOW_DIR — wrong directory?" >&2
	exit 2
fi

fail=0
examined=0
skipped=0
blocks=0

for wf in "${workflows[@]}"; do
	if head -5 "$wf" | grep -qiE 'generated|DO NOT EDIT|gh aw'; then
		skipped=$((skipped + 1))
		continue
	fi
	examined=$((examined + 1))

	# Walk the file in awk rather than with a regex over the whole text: a
	# `run:` block is delimited by INDENTATION, so where it ends cannot be
	# matched, only tracked. Emits one record per offending block.
	#
	# LITERAL BLOCKS ONLY — `run: |` and `run: |-`. `run: >` is a FOLDED
	# scalar: YAML joins its lines with spaces, so however many lines it
	# occupies the shell receives ONE command, and counting its physical
	# lines is simply the wrong question. rust.yml's pg-suite steps are the
	# real example — a single `cargo test … -- pg_a pg_b …` wrapped over four
	# lines for legibility, which an earlier draft of this gate reported as
	# "4 lines of shell". A plain `run: cmd` is likewise never a finding,
	# whatever its length.
	out=$(
		awk -v max="$MAX_LINES" '
			function flush() {
				if (!inblk) return
				reason = ""
				if (cf) reason = "control flow (" cfkw ")"
				else if (n > max) reason = n " lines of shell (max " max ")"
				if (reason != "") printf "%d\t%s\n", start, reason
				inblk = 0
			}
			# A line at or left of the run: key indentation closes the block.
			inblk {
				if ($0 ~ /^[[:space:]]*$/) next
				match($0, /^[ ]*/)
				if (RLENGTH <= ind) { flush() }
				else {
					line = $0
					sub(/^[[:space:]]+/, "", line)
					if (line ~ /^#/) next
					n++
					# Control flow at the START of a logical line. Anchored so
					# a bare `--if-present` or a word like `format` cannot
					# match, and so `fi`/`done`/`esac` are not counted twice.
					if (line ~ /^(if|for|while|until|case)[[:space:](]/ ||
					    line ~ /^[A-Za-z_][A-Za-z0-9_]*[[:space:]]*\(\)[[:space:]]*\{?[[:space:]]*$/) {
						cf = 1
						if (cfkw == "") { split(line, w, /[[:space:](]/); cfkw = w[1] }
					}
					next
				}
			}
			/^[[:space:]]*(-[[:space:]]+)?run:[[:space:]]*\|-?[[:space:]]*$/ {
				# The baseline is the COLUMN OF `run:`, not the leading
				# whitespace. For a `- run: |` step the dash is not
				# whitespace, so leading-space width would be 6 where the
				# key sits at 8 — and the sibling `env:` at 8 would then
				# read as block body and be counted as shell.
				ind = index($0, "run:") - 1
				inblk = 1; n = 0; cf = 0; cfkw = ""; start = NR
				next
			}
			END { flush() }
		' "$wf"
	)

	# Count every multi-line block, offending or not, for control #2.
	blocks=$((blocks + $(grep -cE '^[[:space:]]*(-[[:space:]]+)?run:[[:space:]]*\|-?[[:space:]]*$' "$wf" || true)))

	[ -n "$out" ] || continue
	while IFS=$'\t' read -r ln reason; do
		[ -n "$ln" ] || continue
		echo "workflow-shell-check: $wf:$ln — $reason; move it to scripts/ci/ (CLAUDE.md, conventions)" >&2
		fail=1
	done <<< "$out"
done

# POSITIVE CONTROL #2: every workflow looking generated, or not one multi-line
# `run:` block anywhere, means the detection broke rather than that the tree is
# clean. A gate that silently examines nothing is worse than no gate — it
# reports success for the thing it stopped checking.
if [ "$examined" -eq 0 ]; then
	echo "workflow-shell-check: every workflow looked generated; nothing was examined" >&2
	exit 2
fi
if [ "$blocks" -eq 0 ]; then
	echo "workflow-shell-check: found no multi-line \`run:\` blocks at all — the block parser is broken, not the tree" >&2
	exit 2
fi

if [ "$fail" -ne 0 ]; then
	echo "workflow-shell-check: extract the shell above into scripts/ci/ and call it with \`run: bash scripts/ci/<name>.sh\`." >&2
	echo "workflow-shell-check: pass workflow values in through \`env:\` rather than interpolating \${{ }} into the script." >&2
	exit 1
fi

echo "workflow-shell-check: ok (${examined} hand-written workflows, ${skipped} generated, ${blocks} inline blocks, all within budget)"
