#!/usr/bin/env bash
# Fold the pass-marker lookup into the gate's run/skip output.
#
# Two inputs, and the asymmetry between them is the whole logic: RAN is what
# the path filter decided, HIT is whether scripts/ci/pass-key.sh's key already
# has a marker. Only `ran AND hit` skips — a miss, an absent key (see that
# script's positive control) or a filter that said "no" all fall through to
# RAN unchanged, so no failure of the marker machinery can turn a gate that
# should run into one that does not.
#
# rust.yml and frontend.yml carried byte-identical copies differing only in
# the name they wrote, which is why that is a parameter — and it is TWO
# parameters, not one: rust.yml's detect job publishes this as `code` while
# frontend.yml publishes it as `frontend`, and `needs.detect.outputs.code`
# is read all over rust.yml, so the output name cannot be folded into the
# human label in the notice.
#
# Inputs (env): GATE (the label in the notice), OUT (the output key, default
# GATE), RAN, HIT, KEY. Outputs ($GITHUB_OUTPUT): <OUT>.
# Run: GATE=rust OUT=code RAN=.. HIT=.. KEY=.. bash scripts/ci/pass-decide.sh
set -euo pipefail

: "${GATE:?GATE must name the gate for the skip notice}"
OUT="${OUT:-$GATE}"

if [ "${RAN:-}" = "true" ] && [ "${HIT:-}" = "true" ]; then
	echo "::notice::$GATE gate skipped: this exact content already passed on this PR (${KEY:-})"
	echo "$OUT=false" >> "$GITHUB_OUTPUT"
else
	echo "$OUT=${RAN:-}" >> "$GITHUB_OUTPUT"
fi
