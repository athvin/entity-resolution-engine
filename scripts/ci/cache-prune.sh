#!/usr/bin/env bash
# Prune the Actions cache to per-prefix budgets, oldest-accessed first.
#
# Inputs (env, set by rust.yml's `cache-prune` job, which documents WHY each
# budget is what it is — the measurements live with the numbers):
#   GH_TOKEN           token with actions:write
#   GITHUB_REPOSITORY  owner/repo
#   BUDGETS            one `prefix budget-MiB` per line
#   MAX_DELETES        upper bound on deletions per run
#
# Run (CI): bash scripts/ci/cache-prune.sh
set -euo pipefail

# Totals by ENUMERATION, not from `actions/cache/usage`. That endpoint is a
# lagging aggregate: measured 2026-10-22 immediately after this job deleted 29
# entries worth ~2.7 GiB, it returned the identical byte count on three reads
# 45s apart while the enumerated sum tracked live writes —
#
#     reported 10.02 GiB   enumerated 7.27 GiB   delta 2.75 GiB
#     reported 10.02 GiB   enumerated 7.33 GiB   delta 2.69 GiB
#     reported 10.02 GiB   enumerated 7.28 GiB   delta 2.74 GiB
#
# i.e. the delta was exactly what had been deleted. Reporting from it would
# print an unchanged before/after and make a working prune look like a no-op —
# a hygiene job whose only output lies about its own effect. The enumeration
# costs one extra paginated sweep.
report() {
	gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?per_page=100" \
		--jq '.actions_caches[].size_in_bytes' |
		awk '{s+=$1; n++}
		     END {printf "%d entries, %d bytes (%.2f GiB enumerated)\n", n, s, s/1073741824}'
}

echo "::group::before"
echo "total: $(report)"
gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?per_page=100" \
	--jq '.actions_caches[] | "\(.size_in_bytes)\t\(.key | split("/")[0])"' |
	awk -F'\t' '{c[$2]++; s[$2]+=$1}
	            END {for (k in c) printf "%7d entries %9.1f MiB  %s\n", c[k], s[k]/1048576, k}' |
	sort -k3 -rn
echo "::endgroup::"

# One pass per capped prefix. Each is independent: a prefix already under
# budget contributes no deletions, which is the correct no-op.
grand_deleted=0
while read -r prefix budget_mib; do
	[ -n "$prefix" ] || continue

	entries=$(mktemp)
	gh api --paginate \
		"repos/$GITHUB_REPOSITORY/actions/caches?per_page=100&key=$prefix&sort=last_accessed_at&direction=asc" \
		--jq '.actions_caches[] | "\(.id)\t\(.size_in_bytes)"' >"$entries"

	total=$(awk -F'\t' '{s+=$2} END {print s+0}' "$entries")
	budget=$((budget_mib * 1048576))
	printf '%s: %d entries, %d MiB (budget %d MiB)\n' \
		"$prefix" "$(wc -l <"$entries")" "$((total / 1048576))" "$budget_mib"

	if [ "$total" -le "$budget" ]; then
		echo "  under budget; nothing to do"
		continue
	fi

	# Choose the victims in one pass, then delete them CONCURRENTLY. A
	# sequential loop is what the first version did and it does not fit in
	# any sane timeout: one DELETE is a separate HTTPS round trip (~150 ms)
	# and the backlog was 5,595 entries / 5,846 MiB, i.e. ~3,400 deletions.
	# Measured: it ran past 10 minutes having cleared only part of it.
	# `xargs -P 8` does 1,586 in 2m00.
	#
	# MAX_DELETES bounds a single run so the job cannot become unbounded
	# work if a backlog is ever huge; it converges over successive runs
	# instead. Anything dropped is reported, never silent.
	remaining_cap=$((MAX_DELETES - grand_deleted))
	if [ "$remaining_cap" -le 0 ]; then
		printf '  deletion cap %d already spent; the next run continues here\n' "$MAX_DELETES"
		continue
	fi

	doomed=$(mktemp)
	awk -F'\t' -v budget="$budget" -v total="$total" -v cap="$remaining_cap" '
	  total <= budget { exit }
	  NR > cap        { exit }
	  { print $1; total -= $2 }
	' "$entries" >"$doomed"
	want=$(wc -l <"$doomed")
	printf '  deleting %d oldest-accessed entries (cap %d remaining)\n' "$want" "$remaining_cap"

	# `|| true` per delete: a concurrent run may have evicted it already,
	# which is not a failure. xargs would otherwise exit 123 and, under
	# `set -e`, fail a job whose whole point is hygiene.
	export GITHUB_REPOSITORY
	xargs -r -P 8 -I{} sh -c \
		'gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/{}" --silent 2>/dev/null || true' \
		<"$doomed"

	grand_deleted=$((grand_deleted + want))
	if [ "$want" -ge "$remaining_cap" ]; then
		printf '  hit the deletion cap with %s still over budget; the next run continues\n' "$prefix"
	fi
done <<<"$BUDGETS"

printf 'deleted %d entries in total\n' "$grand_deleted"
echo "total: $(report)"
