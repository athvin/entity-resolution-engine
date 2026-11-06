#!/usr/bin/env bash
#
# worktree-audit.sh — decide which git worktrees are finished, without breaking
# a running session or destroying unpushed work.
#
# READ-ONLY. It prints a verdict per worktree and, for the ones that are
# provably finished, the `git worktree remove` line to run. It never removes
# anything itself: the cost of a wrong removal is another session's work, and
# that is not a cost a script should be allowed to pay on its own.
#
# Why this exists rather than the four commands it replaces. The procedure in
# CLAUDE.md used to say: find live sessions with
# `lsof -a -p <claude pid> -d cwd`, resolve the worktree's HEAD to a pull
# request, and remove the ones whose PR is closed. Each of those steps is
# right, and the first is nowhere near sufficient. Measured on this clone with
# 19 worktrees open:
#
#   - 15 worktrees had a commit land inside the previous 22 minutes.
#   - Only 7 were the cwd of any `claude` process.
#
# The gap is the dominant working pattern here: a session whose cwd is the
# PRIMARY checkout drives its worktree with `git -C <path>`. It holds no file
# descriptor in that directory, so `lsof` cannot see it. One worktree committed
# 12 seconds before the audit evaluated it as unattached, and another moved its
# HEAD between two commands of the same audit run.
#
# So liveness is read from the repository, not the process table: HEAD movement
# during the audit, and how recently the last commit landed. A session that is
# working will trip one of those even when it is invisible to `lsof`.
#
# Usage:
#   scripts/worktree-audit.sh              # verdicts, no sizes (fast)
#   scripts/worktree-audit.sh --size       # add du per worktree (slow: minutes)
#   scripts/worktree-audit.sh --idle-mins N  # how old the last commit must be
#                                            # before "idle" (default 60)
set -euo pipefail

idle_mins=60
want_size=0
while (($#)); do
	case "$1" in
	--size) want_size=1 ;;
	--idle-mins)
		shift
		idle_mins=${1:?--idle-mins needs a number}
		;;
	-h | --help)
		sed -n '3,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		echo "unknown argument: $1" >&2
		exit 2
		;;
	esac
	shift
done

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$here"

# The primary checkout, which is never a candidate however the script is
# invoked. Deriving it from the script's own location is wrong: run from inside
# a worktree, that yields the WORKTREE, which then excludes itself from the
# audit and offers the primary checkout up for removal instead. `--git-common-dir`
# is the primary's `.git` from anywhere in the clone.
primary=$(cd "$(dirname "$(git rev-parse --git-common-dir)")" && pwd)

# Every worktree except the primary checkout, which is never a candidate.
#
# `while read` rather than `mapfile`, `date +%s` rather than `EPOCHSECONDS`, and
# index-aligned arrays rather than `declare -A`: `/usr/bin/env bash` on a stock
# macOS is 3.2, where all three of those are runtime failures that `bash -n`
# does not catch. The other scripts here use none of them either.
paths=()
while IFS= read -r line; do
	[[ -n $line ]] && paths+=("$line")
done < <(git worktree list --porcelain |
	awk '/^worktree /{ print substr($0, 10) }' |
	grep -vFx "$primary" || true)

if ((${#paths[@]} == 0)); then
	echo "no worktrees besides the primary checkout"
	exit 0
fi

# cwd of every live claude process. Still worth reading — it is proof of
# attachment when present, just not proof of absence when missing.
live_cwds=""
for pid in $(pgrep -x claude 2>/dev/null || true); do
	cwd=$(lsof -a -p "$pid" -d cwd 2>/dev/null | awk 'NR>1 { print $NF }')
	[[ -n $cwd ]] && live_cwds+="$cwd"$'\n'
done

# First HEAD sample, index-aligned with `paths`. The second is taken after the
# slow per-worktree work, so the window a committing session has to reveal
# itself is as wide as the audit.
head_first=()
for p in "${paths[@]}"; do
	head_first+=("$(git -C "$p" rev-parse HEAD 2>/dev/null || echo '?')")
done

now=$(date +%s)
removable=()
printf '%-34s %-9s %-7s %-18s %s\n' WORKTREE HEAD DIRTY LAST-COMMIT VERDICT
i=-1
for p in "${paths[@]}"; do
	i=$((i + 1))
	name=$(basename "$p")
	sha=${head_first[$i]}
	short=${sha:0:8}

	# `--no-optional-locks` is load-bearing, not tidiness. A plain
	# `git status` REFRESHES THE INDEX, so it rewrites the very mtime the
	# activity check below reads — the audit would stamp every worktree as
	# active 0 minutes ago and keep all of them, including genuinely finished
	# ones. Observed exactly that: a worktree whose markers had been backdated
	# to five days earlier came back "git activity 0m ago" because this line
	# had already touched it.
	dirty=$(git --no-optional-locks -C "$p" status --porcelain 2>/dev/null | wc -l | tr -d ' ')
	last_rel=$(git -C "$p" log -1 --format='%cr' 2>/dev/null || echo "?")

	# Liveness is LOCAL ACTIVITY, not the age of the commit HEAD points at. A
	# worktree cut from a week-old commit is brand new; one cut from a commit
	# that landed a minute ago may never be touched again. Both read backwards
	# if you measure the commit.
	#
	# What does track activity is the per-worktree git directory: its `index`
	# is rewritten by status, add, commit, checkout — any operation a session
	# performs in that tree. The HEAD reflog is the same signal from the other
	# side. Take whichever is more recent.
	gitdir=$(git -C "$p" rev-parse --absolute-git-dir 2>/dev/null || echo "")
	act_epoch=0
	if [[ -n $gitdir ]]; then
		for probe in "$gitdir/index" "$gitdir/logs/HEAD"; do
			[[ -f $probe ]] || continue
			t=$(stat -f %m "$probe" 2>/dev/null || stat -c %Y "$probe" 2>/dev/null || echo 0)
			((t > act_epoch)) && act_epoch=$t
		done
	fi
	age_mins=$(((now - act_epoch) / 60))

	size=""
	if ((want_size)); then
		size=" size=$(du -sg "$p" 2>/dev/null | cut -f1)GB"
	fi

	# The local guards first, because they are free and they decide most of the
	# fleet. Each one alone is enough to keep the worktree, so a worktree that
	# trips any of them never costs a network round trip — which is what makes
	# the audit usable with twenty worktrees open instead of timing out partway
	# through and leaving half the fleet unjudged.
	reasons=()
	[[ $p == "$here" ]] && reasons+=("this audit is running from it")
	printf '%s' "$live_cwds" | grep -qxF "$p" && reasons+=("a live claude process has it as cwd")
	((dirty > 0)) && reasons+=("$dirty uncommitted path(s) — not landed anywhere")
	((age_mins < idle_mins)) && reasons+=("git activity ${age_mins}m ago, under the ${idle_mins}m idle floor")

	pr_state=""
	pr_merged=""
	pr_num=""
	if ((${#reasons[@]} == 0)); then
		# Quiet, clean and unattached — now it is worth asking GitHub. A 422
		# means the commit is not on the remote at all, which is unlanded work
		# and the strongest reason to keep. `--jq` over a captured string,
		# never `grep -q` on a pipe, per the house rule.
		pr_json=$(timeout 15 gh api "repos/{owner}/{repo}/commits/$sha/pulls" 2>/dev/null || true)
		if [[ -n $pr_json && $pr_json != *'"status": "422"'* && $pr_json != *'"status":"422"'* ]]; then
			pr_num=$(printf '%s' "$pr_json" | jq -r '.[0].number // empty' 2>/dev/null || true)
			pr_state=$(printf '%s' "$pr_json" | jq -r '.[0].state // empty' 2>/dev/null || true)
			pr_merged=$(printf '%s' "$pr_json" | jq -r '(.[0].merged_at != null) // empty' 2>/dev/null || true)
		fi
		[[ -z $pr_state ]] && reasons+=("no PR resolves for $short — never pushed, so this is unlanded work")
		[[ $pr_state == "open" ]] && reasons+=("PR #$pr_num is still open")

		# Second HEAD sample, taken last: did a session commit while we worked?
		if [[ $(git -C "$p" rev-parse HEAD 2>/dev/null || echo "?") != "$sha" ]]; then
			reasons+=("HEAD MOVED during this audit — a session is committing here")
		fi
	fi

	if ((${#reasons[@]} == 0)); then
		printf '%-34s %-9s %-7s %-18s %s\n' "$name" "$short" "$dirty" "$last_rel" \
			"REMOVABLE — PR #$pr_num $pr_state (merged=$pr_merged)$size"
		removable+=("$p")
	else
		printf '%-34s %-9s %-7s %-18s %s\n' "$name" "$short" "$dirty" "$last_rel" "KEEP$size"
		for reason in "${reasons[@]}"; do
			printf '%*s- %s\n' 34 "" "$reason"
		done
	fi
done

echo
if ((${#removable[@]} == 0)); then
	echo "Nothing is removable. That is a normal answer while sessions are running —"
	echo "re-run when the fleet is idle rather than reaching for --force."
	exit 0
fi

echo "Finished worktrees (${#removable[@]}). Review, then run:"
for p in "${removable[@]}"; do
	echo "  git worktree remove $p"
done
echo "  git worktree prune"
echo
echo "A refusal here because the tree is dirty is information, not an obstacle:"
echo "find out what the change was before considering --force."
