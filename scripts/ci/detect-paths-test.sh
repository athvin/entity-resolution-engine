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

ALL='code=true erd=true drift=true empty=false'
NONE='code=false erd=false drift=false empty=false'
# A PR that changes nothing: no compile job, and `empty pull request` fails so
# the PR is not green. Distinct from NONE, which is a real (exempt) change.
EMPTY='code=false erd=false drift=false empty=true'

# --- the gate MUST run -----------------------------------------------------
expect 'a rust source change' "$ALL" 'rust/crates/er-match/src/lib.rs'
expect 'a manifest change' "$ALL" 'rust/Cargo.toml'
expect 'an entity yaml' "$ALL" 'entities/orgs/entity.yaml'
expect 'the frontend' "$ALL" 'frontend/src/app/page.tsx'
expect 'marketing page' "$NONE" 'frontend/src/app/(marketing)/page.tsx'
expect 'marketing layout' "$NONE" 'frontend/src/app/(marketing)/layout.tsx'
expect 'marketing demo' "$NONE" 'frontend/src/components/marketing/demo/flow-demo.tsx'
expect 'marketing browser coverage' "$NONE" 'frontend/tests/functional/marketing-interactions.spec.ts'
expect 'marketing visual baseline' "$NONE" 'frontend/tests/functional/marketing.spec.ts-snapshots/hero.png'
expect 'marketing social image' "$NONE" 'frontend/src/app/opengraph-image.png'
expect 'marketing and shared shell' "$ALL" 'frontend/src/app/(marketing)/page.tsx' 'frontend/src/app/layout.tsx'
expect 'marketing and backend' "$ALL" 'frontend/src/components/marketing/hero.tsx' 'rust/Cargo.lock'
for path in frontend/src/app/globals.css frontend/src/proxy.ts frontend/package.json frontend/pnpm-lock.yaml frontend/src/lib/api/openapi.json frontend/src/lib/entities/bindings/Entity.ts frontend/drizzle/meta/0000_snapshot.json frontend/src/components/settings/webhooks-content.tsx frontend/src/components/marketing-shared.tsx 'frontend/src/app/(marketing)/api/route.ts' frontend/tests/functional/marketing.spec.ts.extra; do
	expect "shared or unrecognized path: $path" "$ALL" "$path"
done
expect 'the skills gate itself' "$ALL" 'scripts/skills-check.sh'
expect 'the classifier itself (fail-safe)' "$ALL" 'scripts/ci/detect-paths.sh'
expect 'this test (fail-safe)' "$ALL" 'scripts/ci/detect-paths-test.sh'
expect 'rust.yml itself' "$ALL" '.github/workflows/rust.yml'
expect 'frontend.yml itself' "$ALL" '.github/workflows/frontend.yml'
# The devcontainer's exemption covers `.devcontainer/` and `devcontainer.yml`
# and must stop there. These two live under `scripts/`, which the regex does not
# cover, and the direction that matters is the FALSE SKIP: were `devcontainer-`
# ever folded into the exemption as a prefix, an edit to a tracked shell script
# would stop running the gate.
expect 'a devcontainer lifecycle script' "$ALL" 'scripts/devcontainer-postcreate.sh'
expect 'the devcontainer detect helper' "$ALL" 'scripts/ci/devcontainer-detect.sh'
expect 'a prose skill ref AND a rust file' "$ALL" \
	'.claude/skills/er-matching/references/invariants.md' 'rust/crates/er-match/src/lib.rs'
expect 'a skill AND a rust file' "$ALL" '.claude/skills/er-matching/SKILL.md' 'rust/Cargo.lock'
expect 'a hook AND a rust file' "$ALL" 'scripts/hooks/aws-write-guard.sh' 'rust/crates/er-match/src/lib.rs'

# --- a pull request that changes nothing -----------------------------------
# The shape `claim-ticket` opens deliberately: a draft PR pushed from an empty
# commit. `base...head` is empty, so no job has anything to compile — but this
# is the DENYLIST detect, and before the guard in `detect_classify` the single
# blank line that `printf '%s\n' ""` emits matched no exemption and classified
# as real code, so every one of those PRs paid the full backend gate.
#
# No path arguments at all is exactly what the workflow passes: `expect` runs
# `printf '%s\n' "$@"`, which with zero args emits one newline, and
# `changed="$(cat)"` strips it — byte-identical to `git diff` finding nothing.
expect 'a pull request that changes nothing' "$EMPTY"

# --- no compile job at all -------------------------------------------------
expect 'a skill body only' "$NONE" '.claude/skills/er-matching/references/invariants.md'
expect 'a skill eval only' "$NONE" '.claude/skills/security/evals.md'
expect 'a prose skill reference' "$NONE" '.claude/skills/er-matching/references/invariants.md'
expect 'several prose skill references' "$NONE" \
	'.claude/skills/er-matching/references/invariants.md' \
	'.claude/skills/frontend-testing/references/tiers-and-thresholds.md'
expect 'terraform only' "$NONE" 'infra/terraform/envs/dev/main.tf'
expect 'a tf ci helper' "$NONE" 'scripts/ci/tf-helpers.sh'
expect 'the aws write-guard hook' "$NONE" 'scripts/hooks/aws-write-guard.sh'
expect 'an agentic workflow' "$NONE" '.github/workflows/ci-triage.md'
expect 'its compiled lock' "$NONE" '.github/workflows/ci-triage.lock.yml'
expect 'infra.yml' "$NONE" '.github/workflows/infra.yml'
# #584's Direction B: a `.devcontainer/`-only PR must not pay the backend gate,
# and must not be blocked by it. Both halves of the exemption, because they are
# separate alternations in the regex.
expect 'the devcontainer image' "$NONE" '.devcontainer/Dockerfile'
expect 'its devcontainer.json' "$NONE" '.devcontainer/devcontainer.json'
expect 'devcontainer.yml' "$NONE" '.github/workflows/devcontainer.yml'
expect 'the whole devcontainer change' "$NONE" \
	'.devcontainer/Dockerfile' '.devcontainer/devcontainer.json' \
	'.github/workflows/devcontainer.yml'
# The Codex bridge. Both sides are symlinks into the authored Claude Code tree,
# so neither is read by anything in `make check` — and `agent-bridge-check.sh`,
# which does read them, runs ungated in the `skills` job. `AGENTS.md` is its own
# alternation rather than being caught by `CLAUDE\.md$`, so it is asserted
# separately. Note it classifies NONE and not `drift=true`: no generator derives
# anything from it, unlike CLAUDE.md, whose skill-inventory table does.
expect 'a bridged skill entry' "$NONE" '.agents/skills/land-pr'
expect 'the whole skills bridge' "$NONE" '.agents/skills/land-pr' '.agents/skills/pr-review'
expect 'AGENTS.md alone' "$NONE" 'AGENTS.md'

# --- `lint` only, for the drift check -------------------------------------
# This is the shape of a real skill PR: the inventory rows regenerate into
# CLAUDE.md and the agent-skills standard, which is what defeated the old
# `.claude/**` exemption. Both targets now live under `.claude/`, so the
# re-admission in DETECT_DRIFT is the only thing keeping this case true.
expect 'a skill + its regenerated tables' \
	'code=false erd=false drift=true empty=false' \
	'.claude/skills/skill-audit/SKILL.md' 'CLAUDE.md' '.claude/skills/skill-writer/references/agent-skills.md'
expect 'CLAUDE.md alone' 'code=false erd=false drift=true empty=false' 'CLAUDE.md'
expect 'the agent-skills standard' 'code=false erd=false drift=true empty=false' \
	'.claude/skills/skill-writer/references/agent-skills.md'
expect 'the diagnostics generator source' \
	'code=false erd=false drift=true empty=false' '.claude/skills/entity-framework/references/entity-framework-design.md'

# --- `entities` only, for the generated ERD --------------------------------
expect 'the generated ERD' 'code=false erd=true drift=false empty=false' '.claude/skills/entity-framework/references/erd.md'
expect 'the ERD plus prose' 'code=false erd=true drift=false empty=false' \
	'.claude/skills/entity-framework/references/erd.md' '.claude/skills/er-matching/references/invariants.md'
expect 'the ERD plus a drift target' \
	'code=false erd=true drift=true empty=false' '.claude/skills/entity-framework/references/erd.md' 'CLAUDE.md'

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
