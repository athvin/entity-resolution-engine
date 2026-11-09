#!/usr/bin/env bash
# The frontend gate's input set. Sourced, not executed — by frontend.yml's
# `filter` step (via frontend-detect.sh) and by its content-addressed `key`
# step, so the two cannot drift apart; they used to carry two copies of this
# regex with a comment asking future editors to keep them identical.
#
# Any path here means "the frontend gate must run when this changes": the app
# itself, the dev-stack scripts the tests shell out to, the workflow, and the
# gate's own extracted scripts — a change to the gate must run the gate.
#
# `pass-key.sh`, `pass-decide.sh` and `pass-gate-test.sh` are SHARED with the
# Rust gate, and they are in this list for the same reason the frontend-only
# ones are: they decide whether this gate runs at all, so a change to them
# that did not run this gate would ship the one kind of bug nothing else
# catches. The Rust side sees them too — `scripts/ci/` is not exempt in
# detect-paths.sh except for the `tf-` prefix — so both gates re-prove
# themselves on a change to either.
# shellcheck disable=SC2034  # consumed by the files that source this
FRONTEND_INCLUDE='^scripts/ci/(successful-base\.py|gate-selection\.sh|test_ci_[^/]+\.py)$|^(frontend/|scripts/frontend-dev/|scripts/ci/frontend-(paths|detect|run)\.sh$|scripts/ci/marketing-(paths|ci-test)\.sh$|scripts/ci/gate-all-legs\.sh$|scripts/ci/pass-(key|decide|gate-test)\.sh$|\.github/workflows/frontend\.yml$)'

# shellcheck source=scripts/ci/marketing-paths.sh
. "$(dirname "${BASH_SOURCE[0]}")/marketing-paths.sh"

# Unrelated backend/docs paths do not widen frontend coverage. Any shared app
# path does. Renames are diffed with --no-renames so both ends are classified.
frontend_classify() {
	local changed relevant app
	changed="$(cat)"
	relevant="$(printf '%s\n' "$changed" | grep -E "$FRONTEND_INCLUDE" || true)"
	if [ -z "$relevant" ]; then
		printf 'frontend=false\nmarketing_only=false\n'
		return
	fi
	app="$(printf '%s\n' "$relevant" | grep -cvE "$MARKETING_INCLUDE" || true)"
	printf 'frontend=true\n'
	if [ "$app" -eq 0 ]; then
		printf 'marketing_only=true\n'
	else
		printf 'marketing_only=false\n'
	fi
}
