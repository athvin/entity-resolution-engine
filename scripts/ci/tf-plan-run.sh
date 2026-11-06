#!/usr/bin/env bash
# infra.yml's plan step: init + plan one stack, read-only, and classify the
# exit. `tf-` prefix on purpose: this is a genuine terraform helper, so its
# edits are DETECT_EXEMPT for the rust gate and DO trigger infra.yml (both
# key on the scripts/ci/tf- prefix).
#
# `-lock=false`: the plan role holds no S3 write and acquiring the lockfile IS
# a write; the apply job's concurrency group is what serialises writes.
# `-refresh=false` is a SECURITY choice, not a speed one — a refreshing plan
# re-reads `aws_secretsmanager_secret_version` (needs GetSecretValue) and the
# CUR export, and this role is assumable from any ref because a plan must run
# on the pull request; see infra.yml for the full argument. Drift is caught at
# APPLY time, which refreshes fully.
# `-detailed-exitcode`: 0 = no changes, 2 = changes, 1 = error — without it an
# erroring plan and an empty one share an exit code.
#
# Inputs (env): DIR. Writes planout/{plan.txt,meta.txt}; exits 1 on a failed
# plan, 0 otherwise.
mkdir -p planout
set +e
terraform -chdir="$DIR" init -input=false > planout/plan.txt 2>&1
code=$?
if [ "$code" -eq 0 ]; then
  terraform -chdir="$DIR" plan \
    -no-color -lock=false -refresh=false -input=false -detailed-exitcode \
    > planout/plan.txt 2>&1
  code=$?
else
  # An init failure leaves its own log in plan.txt, so the comment
  # shows why rather than an empty diff. Normalised to 1 = error.
  code=1
fi
set -e

tail -c 60000 planout/plan.txt
bash scripts/ci/tf-plan-meta.sh planout/plan.txt "$code" > planout/meta.txt
cat planout/meta.txt
if [ "$code" -eq 1 ]; then
  echo "::error::terraform plan failed for $DIR"
  exit 1
fi
