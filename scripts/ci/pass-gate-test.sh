#!/usr/bin/env bash
# Self-test for the content-addressed pass gate — scripts/ci/pass-key.sh and
# scripts/ci/pass-decide.sh, which together decide whether the Rust and
# frontend gates run at all.
#
# Same reason detect-paths-test.sh exists, one step further down the chain. A
# wrong answer here is not a wasted run: it is a REQUIRED GATE SKIPPED on a
# change it would have caught, on the strength of a green earned by different
# content. So the assertions below are weighted toward proving that every
# failure mode falls through to "run the gate":
#
#   - only `ran AND hit` may skip; the other three combinations must not;
#   - an unknown gate must fail loudly rather than emit a usable key;
#   - a key must change when a keyed path's CONTENT changes, and must not
#     change when a path the gate does not read changes.
#
# The last pair is the one no amount of reading the script proves, because it
# depends on the regexes in detect-paths.sh and frontend-paths.sh agreeing with
# what the gates actually read. It is asserted here against a real git tree.
#
# It covers BOTH gates but runs in rust.yml's ungated `workflow pins (S9)`
# job, next to detect-paths-test.sh, because that is the one job no path
# filter and no pass marker can skip.
#
# WRITES TO THE OBJECT DATABASE, NOT TO THE WORKING TREE. The probe commits
# are built in a temporary index (`GIT_INDEX_FILE`) and never referenced, so
# they are unreachable objects that `git gc` collects; no branch, tag or ref
# is created and no tracked file is modified. Needs only a depth-1 checkout.
#
# Run: bash scripts/ci/pass-gate-test.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2

pass=0
fail=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

ok() {
	pass=$((pass + 1))
	printf 'ok   %s\n' "$1"
}
no() {
	fail=$((fail + 1))
	printf 'FAIL %s\n       want: %s\n        got: %s\n' "$1" "$2" "$3"
}

check() {
	local name=$1 want=$2 got=$3
	[ "$got" = "$want" ] && ok "$name" || no "$name" "$want" "$got"
}

# --- pass-decide: only `ran AND hit` skips ---------------------------------
decide() {
	local ran=$1 hit=$2
	: > "$tmp/out"
	GITHUB_OUTPUT="$tmp/out" GATE=rust OUT=code RAN="$ran" HIT="$hit" KEY=k \
		bash scripts/ci/pass-decide.sh > /dev/null 2>&1
	cat "$tmp/out"
}

check 'decide: ran+hit skips' 'code=false' "$(decide true true)"
check 'decide: ran+miss RUNS' 'code=true' "$(decide true false)"
check 'decide: not-ran+hit stays false' 'code=false' "$(decide false true)"
check 'decide: not-ran+miss stays false' 'code=false' "$(decide false false)"
# An absent key (pass-key's positive control fired, or the `if:` skipped the
# step) leaves HIT empty. That must RUN, never skip.
check 'decide: empty HIT RUNS' 'code=true' "$(decide true '')"

# The output name is a separate parameter from the label, because rust.yml
# publishes this as `code` and frontend.yml as `frontend`. Defaulting OUT to
# GATE is what frontend.yml relies on.
: > "$tmp/out"
GITHUB_OUTPUT="$tmp/out" GATE=frontend RAN=true HIT=true KEY=k \
	bash scripts/ci/pass-decide.sh > /dev/null 2>&1
check 'decide: OUT defaults to GATE' 'frontend=false' "$(cat "$tmp/out")"

# --- pass-key: a gate it does not know must not produce a key --------------
: > "$tmp/out"
GITHUB_OUTPUT="$tmp/out" GATE=not-a-gate bash scripts/ci/pass-key.sh > /dev/null 2>&1
code=$?
check 'key: unknown gate exits 2' '2' "$code"
check 'key: unknown gate wrote no key' '' "$(cat "$tmp/out")"

# --- pass-key: both gates produce a key, and they differ -------------------
keyfor() {
	: > "$tmp/k"
	GITHUB_OUTPUT="$tmp/k" GATE="$1" bash scripts/ci/pass-key.sh > /dev/null 2>&1
	sed -n 's/^key=//p' "$tmp/k"
}

rk="$(keyfor rust)"
fk="$(keyfor frontend)"
case $rk in
ci-pass-rust-????????????????????????????????????????) ok 'key: rust key is prefixed and 40 hex' ;;
*) no 'key: rust key is prefixed and 40 hex' 'ci-pass-rust-<40>' "$rk" ;;
esac
case $fk in
ci-pass-frontend-????????????????????????????????????????) ok 'key: frontend key is prefixed and 40 hex' ;;
*) no 'key: frontend key is prefixed and 40 hex' 'ci-pass-frontend-<40>' "$fk" ;;
esac

# --- pass-key is CONTENT-addressed, not path-addressed ---------------------
# The key hashes `git ls-tree`, which carries each path's blob OID, so a
# committed content change must move it — but only for the gate that reads the
# path. That is the whole claim the skip rests on.
#
# ASSERTED BY DRIVING pass-key.sh ITSELF, via PASS_KEY_REF, against throwaway
# commits built in a temporary index. The working tree is never touched and no
# ref is created. Re-deriving the filter here instead would be a test with its
# own copy of the regex — able to pass while the gate keys something else,
# which is exactly what detect-paths.sh's header warns against.
blob="$(printf 'probe\n' | git hash-object -w --stdin)"
base_tree="$(git rev-parse HEAD^{tree})"

# probe <path> -> commit sha of HEAD's tree with <path> added or replaced
probe() {
	GIT_INDEX_FILE="$tmp/idx" git read-tree "$base_tree"
	GIT_INDEX_FILE="$tmp/idx" git update-index --add --cacheinfo "100644,$blob,$1"
	git commit-tree "$(GIT_INDEX_FILE="$tmp/idx" git write-tree)" -p HEAD -m probe
}

keyat() { # <gate> <ref> -> the key pass-key.sh emits for that tree
	: > "$tmp/k"
	GITHUB_OUTPUT="$tmp/k" GATE="$1" PASS_KEY_REF="$2" \
		bash scripts/ci/pass-key.sh > /dev/null 2>&1
	sed -n 's/^key=//p' "$tmp/k"
}

differs() {
	local name=$1 a=$2 b=$3
	if [ -n "$a" ] && [ -n "$b" ] && [ "$a" != "$b" ]; then ok "$name"; else
		no "$name" "two different non-empty keys" "$a / $b"
	fi
}

# A path the Rust gate reads must move the rust key — and must not move the
# frontend one.
c_rust="$(probe 'rust/crates/er-match/src/zz_probe.rs')"
differs 'key: a new rust/ file moves the rust key' "$rk" "$(keyat rust "$c_rust")"
check 'key: a rust/ file does NOT move the frontend key' \
	"$fk" "$(keyat frontend "$c_rust")"

# The mirror: a frontend path must move the frontend key and not the rust one.
# `code=false` for a frontend-only PR is false here — `make check` reads
# frontend/ — so this is the asymmetry the two filters encode, asserted.
c_front="$(probe 'frontend/src/app/zz_probe.ts')"
differs 'key: a new frontend/ file moves the frontend key' \
	"$fk" "$(keyat frontend "$c_front")"

# An exempt path the Rust gate provably never reads must leave the rust key
# alone — that is the whole point of the skip.
c_docs="$(probe 'docs/zz_probe.md')"
check 'key: an exempt docs/ file does NOT move the rust key' \
	"$rk" "$(keyat rust "$c_docs")"

# But a COUPLED exempt path must still move it, or a change to the generated
# ERD could ride a marker earned before it changed. This is the assertion that
# catches dropping `|| $2 ~ co` from the rust filter.
c_erd="$(probe 'docs/erd.md')"
differs 'key: a coupled docs/erd.md DOES move the rust key' \
	"$rk" "$(keyat rust "$c_erd")"
c_claude="$(probe 'CLAUDE.md')"
differs 'key: coupled CLAUDE.md DOES move the rust key' \
	"$rk" "$(keyat rust "$c_claude")"

echo "---"
echo "pass=$pass fail=$fail"
# A run that asserted nothing is a failure, not a pass — same reason
# detect-paths-test.sh and skills-check.sh fail on zero.
if [ "$pass" -eq 0 ]; then
	echo "FAIL asserted nothing"
	exit 2
fi
[ "$fail" -eq 0 ] || exit 1
echo "pass-gate: ok"
