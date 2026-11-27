#!/usr/bin/env bash
# The stale-tree guard (#1307): flag a pull request whose files return to an
# older version of themselves on main.
#
# WHY. #1286 (2026-11-27) was one commit whose PARENT was a fresh main but whose
# TREE came from an old worktree, so 75 files went back to their versions of
# three days before. That undid seven merged pull requests: most of the
# cinematic marketing epic, and #1279's Parquet export. The squash was CLEAN,
# the old code was self-consistent, and every gate passed. #1310 nearly did the
# same that day. A diff against the parent reads as a feature that also deletes
# things. What gives it away is that each file's new content is a version main
# already had and moved past.
#
# HOW. For every file the pull request changes against its merge base, walk that
# file's history on main, newest first, up to STALE_TREE_DEPTH commits.
# - If the pull request's blob equals the file's blob after any commit OLDER than
#   its latest, the pull request returns the file to that version, and so undoes
#   every later commit to it.
# - A file brought back with the content it had before main deleted it is the same
#   finding.
# - Deletions alone are not findings. Removing a file is ordinary, and a stale
#   tree always carries modified files as well.
#
# SEVERITY. A stale tree moves many files back at once: #1286 returned dozens,
# all to versions from the same few days. One or two files returning is
# usually a deliberate partial revert (#1302 took back #877's metric filter in
# two operations.tf files), or a regenerated screenshot whose pixels happen to
# match an old baseline byte for byte (#1298's CTA baseline). So:
# - STALE_TREE_MIN_FILES (default 3) source files returning fails the check;
# - fewer only warns;
# - a screenshot baseline is reported but never counted toward the threshold.
# All three of those were replayed from main's history when this was written.
#
# Deliberate reverts are exempt: a title starting "Revert" (git's own wording),
# or the `revert-ok` label. Not a required check yet; the order for promoting a
# check is in the ci-and-supply-chain skill.
#
# Inputs (env): BASE_SHA and HEAD_SHA. Optional: PR_TITLE, PR_LABELS (comma
# separated), STALE_TREE_DEPTH (default 60), STALE_TREE_MIN_FILES (default 3),
# GITHUB_STEP_SUMMARY, and
# STALE_TREE_ALLOW_SHALLOW=1 for a local clone that is deep but still marked
# shallow.
# Exit: 0 clean, exempt or warned; 1 a stale tree; 2 it could not look.
# Run locally:
#   BASE_SHA=$(git rev-parse origin/main) HEAD_SHA=$(git rev-parse HEAD) \
#     bash scripts/ci/stale-tree-guard.sh
set -euo pipefail

# stale_tree_findings <base> <head>
# Prints one line per finding: <path> TAB <commit whose version it returns to>
# TAB <commits it undoes, comma separated, newest first>. Then a final line
# `examined=<files looked at>`.
stale_tree_findings() {
	local base=$1 head=$2 depth=${STALE_TREE_DEPTH:-60}
	local merge_base examined=0 path new cur commit blob undone
	merge_base=$(git merge-base "$base" "$head")
	while IFS= read -r -d '' path; do
		examined=$((examined + 1))
		new=$(git rev-parse -q --verify "$head:$path" 2>/dev/null || true)
		[ -n "$new" ] || continue
		cur=$(git rev-parse -q --verify "$merge_base:$path" 2>/dev/null || true)
		[ "$new" != "$cur" ] || continue
		undone=""
		while IFS= read -r commit; do
			blob=$(git rev-parse -q --verify "$commit:$path" 2>/dev/null || true)
			# The newest commit holds main's current version (or deleted the
			# file); only an OLDER version matching is a return.
			if [ -n "$undone" ] && [ "$blob" = "$new" ]; then
				printf '%s\t%s\t%s\n' "$path" "$commit" "$undone"
				break
			fi
			undone="${undone:+$undone,}$commit"
		done < <(git log --format=%H -n "$depth" "$merge_base" -- "$path")
	done < <(git diff -z --no-renames --name-only "$merge_base" "$head")
	printf 'examined=%s\n' "$examined"
}

# An annotation's message is one line, and `%` starts an escape.
annotate() {
	local text=${1//%/%25}
	printf '%s\n' "${text//$'\n'/ }"
}

main() {
	: "${BASE_SHA:?BASE_SHA is required}" "${HEAD_SHA:?HEAD_SHA is required}"
	case "${PR_TITLE:-}" in
	Revert*)
		echo "stale-tree guard: a revert by its title; exempt."
		return 0
		;;
	esac
	case ",${PR_LABELS:-}," in
	*,revert-ok,*)
		echo "stale-tree guard: labelled revert-ok; exempt."
		return 0
		;;
	esac
	# A shallow clone has no history to compare against, and would report
	# every pull request clean. A local clone deepened since (still marked
	# shallow) can say so with STALE_TREE_ALLOW_SHALLOW=1; CI never sets it.
	if [ "$(git rev-parse --is-shallow-repository)" = "true" ] && [ "${STALE_TREE_ALLOW_SHALLOW:-}" != "1" ]; then
		echo "::error::stale-tree guard needs the full history: check out with fetch-depth 0."
		return 2
	fi

	local out examined changed findings summary=${GITHUB_STEP_SUMMARY:-/dev/null}
	out=$(stale_tree_findings "$BASE_SHA" "$HEAD_SHA")
	examined=$(printf '%s\n' "$out" | sed -n 's/^examined=//p')
	changed=$(git diff --no-renames --name-only "$(git merge-base "$BASE_SHA" "$HEAD_SHA")" "$HEAD_SHA" | wc -l | tr -d ' ')
	# A guard that looked at nothing passes everything. Refuse that, rather
	# than report it as clean.
	if [ "$changed" -gt 0 ] && [ "${examined:-0}" -eq 0 ]; then
		echo "::error::stale-tree guard examined none of the $changed changed files."
		return 2
	fi
	findings=$(printf '%s\n' "$out" | sed '/^examined=/d')
	if [ -z "$findings" ]; then
		echo "stale-tree guard: none of $examined changed file(s) returns to an older main version."
		return 0
	fi

	# Screenshot baselines are regenerated, not written, and can land on an
	# old version's exact bytes; they are reported, never counted.
	local count=0 sources path version undone subject list level
	sources=$(printf '%s\n' "$findings" | cut -f1 | grep -cvE -- '-snapshots/|\.(png|jpe?g|webp|gif)$' || true)
	level=warning
	[ "$sources" -ge "${STALE_TREE_MIN_FILES:-3}" ] && level=error
	{
		echo "### Files returned to an older version of themselves"
		echo
		echo "Each file below now matches a version main had and moved past, so merging undoes the later commits named beside it. A tree from an old worktree committed on a fresh main looks like this, many files at once (#1286)."
		echo
		echo "| File | Returns to | Undoes |"
		echo "|---|---|---|"
	} >>"$summary"
	while IFS=$'\t' read -r path version undone; do
		count=$((count + 1))
		subject=$(git log -1 --format='%h %s' "$version")
		list=$(for commit in ${undone//,/ }; do git log -1 --format='%h %s' "$commit"; done | paste -sd ';' - | sed 's/;/; /g')
		annotate "::$level file=$path::$path returns to its version from $subject, undoing: $list"
		printf '| `%s` | %s | %s |\n' "$path" "$subject" "$list" >>"$summary"
	done <<<"$findings"
	echo "stale-tree guard: $count of $examined changed file(s) return to an older main version ($sources of them source files)."
	if [ "$level" = warning ]; then
		echo "Fewer than ${STALE_TREE_MIN_FILES:-3} source files: a partial revert or a regenerated artifact, so this only warns. Check the list anyway."
		return 0
	fi
	echo "That is the stale-tree shape. Rebuild the commit on today's main (\`git merge-tree --write-tree\` with the branch's real base), or, if this is a deliberate revert, title it \"Revert …\" or add the revert-ok label."
	return 1
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
	main "$@"
fi
