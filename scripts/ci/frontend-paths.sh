#!/usr/bin/env bash
# The frontend gate's input set. Sourced, not executed — by frontend.yml's
# `filter` step (via frontend-detect.sh) and by its content-addressed `key`
# step, so the two cannot drift apart; they used to carry two copies of this
# regex with a comment asking future editors to keep them identical.
#
# Any path here means "the frontend gate must run when this changes": the app
# itself, the dev-stack scripts the tests shell out to, the workflow, and the
# gate's own extracted scripts — a change to the gate must run the gate.
# shellcheck disable=SC2034  # consumed by the files that source this
FRONTEND_INCLUDE='^(frontend/|scripts/frontend-dev/|scripts/ci/frontend-paths\.sh$|scripts/ci/frontend-detect\.sh$|scripts/ci/gate-all-legs\.sh$|\.github/workflows/frontend\.yml$)'
