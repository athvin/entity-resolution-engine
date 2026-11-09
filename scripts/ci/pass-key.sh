#!/usr/bin/env bash
# The content-addressed pass key, for both gates that have one.
#
# A PR whose tested tree is byte-identical — over every path the gate reads —
# to a tree that already went green ON THIS PR skips the gate. That is the
# common rebase: `main` moved under the PR but only through paths this gate
# does not read (or not at all), so the re-run would prove the same thing
# twice. It also covers re-pushing the same content with reworded or squashed
# commits.
#
# The key is computed from the CHECKED-OUT MERGE COMMIT — the tree the jobs
# actually test — not from the PR head, so a moved base whose change does
# reach a gated path changes the key and the gate runs.
#
# WHY THIS IS SAFE TO TRUST:
#   - Markers are written only by `record-pass` on a `pull_request` run, after
#     every required job succeeded. Actions caches written from a PR are
#     scoped to that PR's ref, so a PR can only ever skip on a result it
#     produced itself — no PR can plant a marker for another.
#   - The ISO WEEK is part of the key, bounding the floating inputs the tree
#     cannot see (stable rustc via rust-toolchain.toml, DuckDB extension
#     downloads, advisory databases) to at most a week.
#   - `merge_group` and `push` never consult markers. Note what that does and
#     does not buy with no merge queue enabled: nothing re-proves a skip
#     BEFORE it lands, so a PR-level skip CAN let a finding reach `main`. What
#     re-proves it is the push-to-main run, which consults no marker — so the
#     skip cannot keep a finding hidden on `main`, only delay it until after
#     the merge.
# Bypass with the `ci:full` label (or a workflow re-run after removing a
# marker) when you want the full gate regardless.
#
# rust.yml and frontend.yml each carried this, and all thirty lines above it,
# as an inline copy. The two differ in exactly one thing — which paths the gate
# reads — so that is the only thing this script branches on, and it resolves
# each gate's filter by SOURCING THE SAME FILE the gate's `filter` step does.
# That is what stops the key and the filter disagreeing: a path the filter
# ignores cannot change the key, and vice versa.
#
# Inputs (env): GATE (rust|frontend), PASS_KEY_REF (default HEAD). Outputs
# ($GITHUB_OUTPUT): key.
# Run: GATE=rust bash scripts/ci/pass-key.sh
#
# PASS_KEY_REF exists for scripts/ci/pass-gate-test.sh and nothing else. No
# call site sets it, so CI always keys the checked-out merge commit. The test
# needs it because the alternative is a test that re-derives this filter — and
# a test with its own copy of the regex can pass while the gate keys something
# different, which is the failure detect-paths.sh's header exists to prevent.
set -euo pipefail

cd "$(dirname "$0")/../.."

REF="${PASS_KEY_REF:-HEAD}"

case "${GATE:?GATE must be set to rust or frontend}" in
rust)
	# Every path EXCEPT the set the filter proves the gate never reads.
	# Keying the whole remaining tree (rather than an allowlist of rust/ and
	# friends) is the fail-safe direction: `make check` also reads frontend/,
	# entities/, .claude/skills/ and scripts/, and a path nobody remembered to
	# list would otherwise be a silent false skip.
	#
	# Keep a path if the gate reads it: not exempt, OR exempt by prefix but
	# individually coupled to a gate (the four `.claude/`+CLAUDE.md files).
	# Dropping the coupled ones would let a change to the generated ERD ride a
	# pass marker earned before it changed.
	# shellcheck source=scripts/ci/detect-paths.sh
	. scripts/ci/detect-paths.sh
	# awk -v unescapes backslashes, turning literal \(marketing\) into a
	# capture group and making this filter disagree with grep. ENVIRON passes
	# the regex bytes intact on both BSD awk (local) and mawk/gawk (CI).
	tree="$(git ls-tree -r "$REF" |
		GATE_EXEMPT="$DETECT_EXEMPT" GATE_COUPLED="$DETECT_COUPLED" \
		awk -F'\t' 'BEGIN {ex=ENVIRON["GATE_EXEMPT"]; co=ENVIRON["GATE_COUPLED"]} $2 !~ ex || $2 ~ co')"
	;;
frontend)
	# The mirror image: an ALLOWLIST, because the frontend gate reads a
	# bounded subtree rather than most of the repo. The inline copy also ran
	# a second `awk` against an always-empty `exempt` regex, which matched
	# every path and so filtered nothing; it is dropped here rather than
	# carried, since a no-op filter reads as a deliberate exclusion.
	# shellcheck source=scripts/ci/frontend-paths.sh
	. scripts/ci/frontend-paths.sh
	tree="$(git ls-tree -r "$REF" |
		GATE_INCLUDE="$FRONTEND_INCLUDE" awk -F'\t' '$2 ~ ENVIRON["GATE_INCLUDE"]')"
	;;
*)
	echo "pass-key: unknown gate '$GATE' (expected rust or frontend)" >&2
	exit 2
	;;
esac

# POSITIVE CONTROL, and note which way it fails. Zero keyed paths means a
# broken checkout or a filter that stopped matching, not a tree with nothing
# in it — both gates read paths that always exist. Emitting NO key is the
# fail-safe response: the `lookup` step is gated on `key != ''`, so an absent
# key means no cache hit, which means the gate RUNS. Failing the step instead
# would block every PR on a diagnostic, and emitting a key over an empty tree
# would be the one outcome that must never happen — every PR colliding on one
# hash and skipping the gate on each other's green.
n="$(printf '%s\n' "$tree" | grep -c . || true)"
printf 'keyed paths: %s\n' "$n"
if [ "${n:-0}" -eq 0 ]; then
	echo "::warning::pass-key: $GATE matched no paths — emitting no key, so the gate runs"
	exit 0
fi

hash="$({
	printf '%s\n' "$tree"
	printf '%s\n' "selection-v2:${SELECTION_KEY:-full}"
	date -u +%G-W%V
} | sha256sum | cut -c1-40)"
echo "key=ci-pass-$GATE-$hash" >> "$GITHUB_OUTPUT"
