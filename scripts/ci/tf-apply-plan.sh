#!/usr/bin/env bash
# infra.yml's apply tier: init, plan to a file, and render the REDACTED text
# the destroy guard and the summary read.
#
# DELIBERATELY NOT FOLDED INTO scripts/ci/tf-plan-run.sh. That script is the
# PR tier's plan and is read-only by construction — `-lock=false` because the
# plan role holds no S3 write, `-refresh=false` because a refreshing plan
# re-reads secret versions with a role assumable from any ref. This one is the
# opposite on both counts by design: it takes the lock and refreshes fully,
# because drift is caught at apply time. Sharing one script would mean a
# parameter for each, and a plan that silently got the wrong one is exactly
# the failure neither tier can afford.
#
# `tfplan` MUST NOT be uploaded as an artifact and `show -json` MUST NOT be
# printed: a saved plan file carries sensitive values in cleartext, while the
# `show -no-color` human form written here redacts them to
# `(sensitive value)`. Only `applyplan.txt` may leave this job.
#
# Inputs (env): DIR. Writes tfplan (consumed by `terraform apply`) and
# applyplan.txt (the redacted text).
# Run: DIR=infra/terraform/envs/dev bash scripts/ci/tf-apply-plan.sh
set -euo pipefail

: "${DIR:?DIR must name the stack directory}"

terraform -chdir="$DIR" init -input=false
terraform -chdir="$DIR" plan -out=tfplan \
	-no-color -input=false -lock-timeout=5m
terraform -chdir="$DIR" show -no-color tfplan > applyplan.txt
