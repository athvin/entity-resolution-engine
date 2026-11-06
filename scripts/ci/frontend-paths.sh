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
FRONTEND_INCLUDE='^(frontend/|scripts/frontend-dev/|scripts/ci/frontend-paths\.sh$|scripts/ci/frontend-detect\.sh$|scripts/ci/gate-all-legs\.sh$|scripts/ci/pass-(key|decide|gate-test)\.sh$|\.github/workflows/frontend\.yml$)'
