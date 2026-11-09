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
# WRITES TO THE OBJECT DATABASE, NOT TO THE WORKING TREE. The probe trees are
# built in a temporary index (`GIT_INDEX_FILE`) and never referenced, so they
# are unreachable objects that `git gc` collects; no branch, tag or ref is
# created and no tracked file is modified. Needs only a depth-1 checkout, and
# no git identity — see the note on `probe` below for why that matters.
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
# TREES built in a temporary index. The working tree is never touched and no
# ref is created. Re-deriving the filter here instead would be a test with its
# own copy of the regex — able to pass while the gate keys something else,
# which is exactly what detect-paths.sh's header warns against.
#
# A TREE, NOT A COMMIT, and that is not a simplification — it is the fix for a
# real CI failure. `git commit-tree` needs a committer identity, and
# `actions/checkout` configures none: the runner's passwd entry has an empty
# name, so git refused with `fatal: empty ident name`. It passes on a laptop,
# where git auto-detects a name from the OS, which is why this was green
# locally and red on the first run. `pass-key.sh` only ever runs
# `git ls-tree -r "$REF"`, and that takes a tree-ish — so no commit object is
# needed, and the test now depends on no ambient git config at all.
blob="$(printf 'probe\n' | git hash-object -w --stdin)"
base_tree="$(git rev-parse HEAD^{tree})"

# probe <path> -> OID of HEAD's tree with <path> added or replaced
probe() {
	GIT_INDEX_FILE="$tmp/idx" git read-tree "$base_tree"
	GIT_INDEX_FILE="$tmp/idx" git update-index --add --cacheinfo "100644,$blob,$1"
	GIT_INDEX_FILE="$tmp/idx" git write-tree
}

# Guard the helper itself. When `probe` failed in CI it returned empty, and
# `keyat` then fell through to HEAD — so the comparisons degraded to HEAD
# against HEAD. `differs` did catch that, but as "two equal keys" rather than
# as "the probe is broken", which is a slower thing to read.
probed() {
	local oid
	oid="$(probe "$1")"
	if [ -z "$oid" ]; then
		echo "FAIL probe helper produced no tree for $1 — cannot assert anything" >&2
		exit 2
	fi
	printf '%s\n' "$oid"
}

keyat() { # <gate> <ref> -> the key pass-key.sh emits for that tree
	: > "$tmp/k"
	GITHUB_OUTPUT="$tmp/k" GATE="$1" PASS_KEY_REF="$2" \
		bash scripts/ci/pass-key.sh > /dev/null 2>&1
	sed -n 's/^key=//p' "$tmp/k"
}

# An empty keyed tree must never receive a reusable pass marker.
empty_tree="$(git hash-object -t tree -w --stdin < /dev/null)"
check 'key: empty Rust tree emits no key' '' "$(keyat rust "$empty_tree")"
check 'key: empty frontend tree emits no key' '' "$(keyat frontend "$empty_tree")"

differs() {
	local name=$1 a=$2 b=$3
	if [ -n "$a" ] && [ -n "$b" ] && [ "$a" != "$b" ]; then ok "$name"; else
		no "$name" "two different non-empty keys" "$a / $b"
	fi
}

differs 'key: selected packages participate in Rust key' \
    "$(SELECTION_KEY='["er-match"]' keyfor rust)" \
    "$(SELECTION_KEY='["er-server"]' keyfor rust)"

# A path the Rust gate reads must move the rust key — and must not move the
# frontend one.
c_rust="$(probed 'rust/crates/er-match/src/zz_probe.rs')"
differs 'key: a new rust/ file moves the rust key' "$rk" "$(keyat rust "$c_rust")"
check 'key: a rust/ file does NOT move the frontend key' \
	"$fk" "$(keyat frontend "$c_rust")"

# Shared app files still affect both gates; isolated marketing files only
# affect the frontend gate. Drive the actual keys so the two filters agree.
c_front="$(probed 'frontend/src/app/zz_probe.ts')"
differs 'key: a new frontend/ file moves the frontend key' \
	"$fk" "$(keyat frontend "$c_front")"
check 'key: ordinary frontend leaves the rust key unchanged' "$rk" "$(keyat rust "$c_front")"
for path in 'frontend/src/app/(marketing)/page.tsx' 'frontend/src/components/marketing/demo/flow-demo.tsx' 'frontend/tests/functional/marketing-interactions.spec.ts' 'frontend/assets/marketing-social.svg'; do
	c_marketing="$(probed "$path")"
	check "key: marketing leaves rust unchanged: $path" "$rk" "$(keyat rust "$c_marketing")"
	differs "key: marketing moves frontend: $path" "$fk" "$(keyat frontend "$c_marketing")"
done

# An exempt path the Rust gate provably never reads must leave the rust key
# alone — that is the whole point of the skip.
for path in frontend/src/lib/api/openapi.json frontend/src/lib/api/openapi.public.json frontend/src/lib/api/schema.d.ts frontend/src/lib/api/envelopes.json frontend/tests/mock-erserver/server.mjs frontend/src/lib/entities/bindings/Entity.ts frontend/drizzle/meta/0000_snapshot.json scripts/ci/rust-select.py scripts/ci/successful-base.py .github/actions/ci-tools/action.yml; do
    differs "key: coupled input moves Rust: $path" "$rk" "$(keyat rust "$(probed "$path")")"
done

c_prose="$(probed '.claude/skills/er-matching/references/zz_probe.md')"
check 'key: an exempt .claude/ file does NOT move the rust key' \
	"$rk" "$(keyat rust "$c_prose")"

# But a COUPLED exempt path must still move it, or a change to the generated
# ERD could ride a marker earned before it changed. This is the assertion that
# catches dropping `|| $2 ~ co` from the rust filter.
c_erd="$(probed '.claude/skills/entity-framework/references/erd.md')"
differs 'key: the coupled generated ERD DOES move the rust key' \
	"$rk" "$(keyat rust "$c_erd")"
c_claude="$(probed 'CLAUDE.md')"
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
