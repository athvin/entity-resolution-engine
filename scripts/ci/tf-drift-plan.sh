#!/usr/bin/env bash
# drift.yml's refresh-only plan: what the account looks like versus what the
# state says, saved twice — once as text for a human, once as JSON for the
# classifier, exactly as the apply tier does.
#
# `driftplan` AND `driftout/plan.json` CARRY ATTRIBUTE VALUES IN CLEARTEXT.
# Neither may be uploaded as an artifact, and the JSON must never be printed —
# there is deliberately no upload step in that workflow. The classifier emits
# only addresses and attribute NAMES; the `show -no-color` text that reaches
# the summary redacts values to `(sensitive value)`. So the only thing this
# script prints is that redacted text.
#
# `-lock=false`: a refresh-only plan writes no state, and acquiring the
# lockfile is itself a write this role is not meant to make.
#
# Not folded into scripts/ci/tf-plan-run.sh or tf-apply-plan.sh for the reason
# spelled out in the latter: the three tiers differ precisely in their lock
# and refresh posture, which is the one thing a shared parameter would make
# easy to get silently wrong.
#
# Inputs (env): DIR. Writes driftplan, driftout/out.txt, driftout/plan.json.
# Run: DIR=infra/terraform/envs/dev bash scripts/ci/tf-drift-plan.sh
set -euo pipefail

: "${DIR:?DIR must name the stack directory}"

mkdir -p driftout
terraform -chdir="$DIR" init -input=false
terraform -chdir="$DIR" plan -refresh-only -out=driftplan \
	-no-color -lock=false -input=false
terraform -chdir="$DIR" show -no-color driftplan > driftout/out.txt
terraform -chdir="$DIR" show -json driftplan > driftout/plan.json
tail -c 60000 driftout/out.txt
