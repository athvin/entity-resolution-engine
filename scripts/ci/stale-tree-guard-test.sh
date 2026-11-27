#!/usr/bin/env bash
# Self-test for stale-tree-guard.sh, the check behind `stale tree guard`.
#
# Same reason as every detector's self-test here: a guard breaks by noticing
# nothing, which reads exactly like a clean pull request. So it builds a small
# repository with the #1286 shape (a tree from an old worktree committed on a
# fresh main) and asserts both directions:
# - that shape fails, naming each returned file and the commit it undoes;
# - one file returning (a deliberate partial revert) only warns, and a
#   screenshot baseline returning never counts;
# - a genuine change on the same files passes;
# - the two exemptions pass;
# - a shallow clone refuses to answer rather than calling everything clean.
#
# Run: bash scripts/ci/stale-tree-guard-test.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2
guard="$PWD/scripts/ci/stale-tree-guard.sh"

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

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
repo="$tmp/repo"
export GIT_AUTHOR_NAME=guard GIT_AUTHOR_EMAIL=guard@example.invalid
export GIT_COMMITTER_NAME=guard GIT_COMMITTER_EMAIL=guard@example.invalid
git init -q -b main "$repo"
commit() {
	git -C "$repo" add -A && git -C "$repo" commit -q -m "$1"
}

mkdir -p "$repo/shots-snapshots"
printf 'v1\n' >"$repo/a.txt"
printf 'x1\n' >"$repo/b.txt"
printf 'd1\n' >"$repo/d.txt"
printf 'e1\n' >"$repo/e.txt"
printf 'png1\n' >"$repo/shots-snapshots/hero.png"
commit "first (#1)"
printf 'v2\n' >"$repo/a.txt"
printf 'e2\n' >"$repo/e.txt"
printf 'png2\n' >"$repo/shots-snapshots/hero.png"
commit "a, e and the baseline move on (#2)"
git -C "$repo" rm -q d.txt
commit "d goes (#3)"
main=$(git -C "$repo" rev-parse HEAD)

# The #1286 shape: a.txt, e.txt and the baseline back to their first
# versions, d.txt back as it was, and b.txt new for real.
git -C "$repo" switch -q -c stale
printf 'v1\n' >"$repo/a.txt"
printf 'd1\n' >"$repo/d.txt"
printf 'e1\n' >"$repo/e.txt"
printf 'png1\n' >"$repo/shots-snapshots/hero.png"
printf 'x2\n' >"$repo/b.txt"
commit "a feature, committed on a stale tree"
stale=$(git -C "$repo" rev-parse HEAD)

# A deliberate partial revert: one file taken back, the rest moving forward.
git -C "$repo" switch -q main
git -C "$repo" switch -q -c partial
printf 'v1\n' >"$repo/a.txt"
printf 'x2\n' >"$repo/b.txt"
commit "take a back, b forward"
partial=$(git -C "$repo" rev-parse HEAD)

# A regenerated baseline that lands on an old version's bytes, and nothing else.
git -C "$repo" switch -q main
git -C "$repo" switch -q -c baseline
printf 'png1\n' >"$repo/shots-snapshots/hero.png"
commit "regenerate the baseline"
baseline=$(git -C "$repo" rev-parse HEAD)

git -C "$repo" switch -q main
git -C "$repo" switch -q -c fresh
printf 'v3\n' >"$repo/a.txt"
printf 'x2\n' >"$repo/b.txt"
commit "a genuine change"
fresh=$(git -C "$repo" rev-parse HEAD)

# guard <dir> <head> [title] [labels] — prints the output, then `code=<exit>`.
guard() {
	local dir=$1 head=$2 title=${3:-feat: something} labels=${4:-} out code
	out=$(cd "$dir" && BASE_SHA=$main HEAD_SHA=$head PR_TITLE=$title PR_LABELS=$labels \
		GITHUB_STEP_SUMMARY="$tmp/summary" bash "$guard" 2>&1)
	code=$?
	printf '%s\ncode=%s\n' "$out" "$code"
}
has() {
	local name=$1 haystack=$2 needle=$3
	case "$haystack" in
	*"$needle"*) ok "$name" ;;
	*) bad "$name" "contains: $needle" "$haystack" ;;
	esac
}
lacks() {
	local name=$1 haystack=$2 needle=$3
	case "$haystack" in
	*"$needle"*) bad "$name" "does not contain: $needle" "$haystack" ;;
	*) ok "$name" ;;
	esac
}

out=$(guard "$repo" "$stale")
has "a stale tree fails" "$out" "code=1"
has "as an error annotation" "$out" "::error file=a.txt::"
has "a returned file is named, with the version it returns to" "$out" "a.txt returns to its version from"
has "and with the commit it undoes" "$out" "a, e and the baseline move on (#2)"
has "a file brought back after its deletion is named" "$out" "d.txt returns to its version from"
has "and with the deletion it undoes" "$out" "d goes (#3)"
has "a returned baseline is still reported" "$out" "shots-snapshots/hero.png returns"
has "but counted apart from the source files" "$out" "(3 of them source files)"
lacks "a genuinely new version is not" "$out" "b.txt returns"
has "the step summary carries the table" "$(cat "$tmp/summary" 2>/dev/null)" '| `a.txt` |'

out=$(guard "$repo" "$partial")
has "one file taken back only warns" "$out" "code=0"
has "as a warning annotation" "$out" "::warning file=a.txt::"
out=$(guard "$repo" "$baseline")
has "a baseline landing on old bytes does not fail" "$out" "code=0"
has "and counts as no source file" "$out" "(0 of them source files)"

out=$(guard "$repo" "$fresh")
has "a genuine change passes" "$out" "code=0"
has "and says what it examined" "$out" "none of 2 changed file(s)"

out=$(guard "$repo" "$stale" 'Revert "a moves on (#2)"')
has "a revert by its title is exempt" "$out" "code=0"
out=$(guard "$repo" "$stale" "feat: x" "enhancement,revert-ok")
has "the revert-ok label exempts" "$out" "code=0"

git clone -q --depth 1 --branch stale "file://$repo" "$tmp/shallow" 2>/dev/null
out=$(guard "$tmp/shallow" "$stale")
has "a shallow clone refuses to answer" "$out" "code=2"

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
