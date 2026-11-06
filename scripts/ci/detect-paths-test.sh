#!/usr/bin/env bash
# Self-test for `detect-paths.sh`, the path classifier `rust.yml`'s `detect` job
# uses to decide which required jobs run.
#
# Same reason `tf-helpers-test.sh` exists: this logic decides whether the backend
# gate runs at all, so getting it wrong is either a 40-minute bill on every prose
# PR or — far worse — a silent skip of the gate on a real code change. Both
# directions are asserted below; the `code=true` cases are the load-bearing ones.
#
# Run: bash scripts/ci/detect-paths-test.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2
# shellcheck source=scripts/ci/detect-paths.sh
. scripts/ci/detect-paths.sh

pass=0
fail=0

# expect <name> <expected-triplet> <path>...
expect() {
	local name=$1 want=$2
	shift 2
	local got
	got="$(printf '%s\n' "$@" | detect_classify | tr '\n' ' ' | sed 's/ $//')"
	if [ "$got" = "$want" ]; then
		pass=$((pass + 1))
		printf 'ok   %s\n' "$name"
	else
		fail=$((fail + 1))
		printf 'FAIL %s\n       want: %s\n        got: %s\n' "$name" "$want" "$got"
	fi
}

ALL='code=true erd=true drift=true'
NONE='code=false erd=false drift=false'

# --- the gate MUST run -----------------------------------------------------
expect 'a rust source change' "$ALL" 'rust/crates/er-match/src/lib.rs'
expect 'a manifest change' "$ALL" 'rust/Cargo.toml'
expect 'an entity yaml' "$ALL" 'entities/orgs/entity.yaml'
expect 'the frontend' "$ALL" 'frontend/src/app/page.tsx'
expect 'the skills gate itself' "$ALL" 'scripts/skills-check.sh'
expect 'the classifier itself (fail-safe)' "$ALL" 'scripts/ci/detect-paths.sh'
expect 'this test (fail-safe)' "$ALL" 'scripts/ci/detect-paths-test.sh'
expect 'rust.yml itself' "$ALL" '.github/workflows/rust.yml'
expect 'frontend.yml itself' "$ALL" '.github/workflows/frontend.yml'
expect 'a doc AND a rust file' "$ALL" 'docs/architecture.md' 'rust/crates/er-match/src/lib.rs'
expect 'a skill AND a rust file' "$ALL" '.claude/skills/er-matching/SKILL.md' 'rust/Cargo.lock'
expect 'a hook AND a rust file' "$ALL" 'scripts/hooks/aws-write-guard.sh' 'rust/crates/er-match/src/lib.rs'

# --- no compile job at all -------------------------------------------------
expect 'a skill body only' "$NONE" '.claude/skills/er-matching/references/invariants.md'
expect 'a skill eval only' "$NONE" '.claude/skills/security/evals.md'
expect 'pure prose docs' "$NONE" 'docs/architecture.md'
expect 'several prose docs' "$NONE" 'docs/architecture.md' 'docs/frontend-testing.md'
expect 'terraform only' "$NONE" 'infra/terraform/envs/dev/main.tf'
expect 'a tf ci helper' "$NONE" 'scripts/ci/tf-helpers.sh'
expect 'the aws write-guard hook' "$NONE" 'scripts/hooks/aws-write-guard.sh'
expect 'an agentic workflow' "$NONE" '.github/workflows/ci-triage.md'
expect 'its compiled lock' "$NONE" '.github/workflows/ci-triage.lock.yml'
expect 'infra.yml' "$NONE" '.github/workflows/infra.yml'

# --- `lint` only, for the drift check -------------------------------------
# This is the shape of a real skill PR: the inventory rows regenerate into
# CLAUDE.md and docs/agent-skills.md, which is what defeated the old
# `.claude/**` exemption.
expect 'a skill + its regenerated tables' \
	'code=false erd=false drift=true' \
	'.claude/skills/skill-audit/SKILL.md' 'CLAUDE.md' 'docs/agent-skills.md'
expect 'CLAUDE.md alone' 'code=false erd=false drift=true' 'CLAUDE.md'
expect 'the agent-skills standard' 'code=false erd=false drift=true' 'docs/agent-skills.md'
expect 'the diagnostics generator source' \
	'code=false erd=false drift=true' 'docs/entity-framework-design.md'

# --- `entities` only, for the generated ERD --------------------------------
expect 'the generated ERD' 'code=false erd=true drift=false' 'docs/erd.md'
expect 'the ERD plus prose' 'code=false erd=true drift=false' 'docs/erd.md' 'docs/architecture.md'
expect 'the ERD plus a drift target' \
	'code=false erd=true drift=true' 'docs/erd.md' 'CLAUDE.md'

echo "---"
echo "pass=$pass fail=$fail"
# A test run that asserted nothing is a failure, not a pass — same reason
# skills-check.sh fails when it examines 0 skills.
if [ "$pass" -eq 0 ]; then
	echo "FAIL asserted nothing"
	exit 2
fi
[ "$fail" -eq 0 ] || exit 1
echo "detect-paths: ok"
