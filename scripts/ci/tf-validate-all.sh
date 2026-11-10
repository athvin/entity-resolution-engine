#!/usr/bin/env bash
# `terraform validate` every stack, including both bootstrap ones.
#
# THIS IS THE ONLY TIER THAT COVERS `bootstrap/organization` AT ALL, and it
# needs no variable value to do so: `terraform validate` does not evaluate
# variables, so the gitignored `terraform.tfvars` that stack depends on (a
# member account's root email, deliberately kept out of version control) is
# not required here. That is also why it cannot join the plan tier, which
# would need the value.
#
# `-backend=false`: validation is a local parse and type check, so it must not
# reach for the shared S3 state — that would need credentials this tier
# deliberately does not have.
#
# Run: bash scripts/ci/tf-validate-all.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

for d in bootstrap/state-backend bootstrap/organization envs/core envs/dev envs/prod; do
	echo "== validate $d =="
	terraform -chdir="infra/terraform/$d" init -backend=false -input=false
	terraform -chdir="infra/terraform/$d" validate -no-color
done

# Mock-provider plans exercise tenant trust/prefix boundaries without AWS.
terraform -chdir=infra/terraform/modules/tenant-runner init -backend=false -input=false
terraform -chdir=infra/terraform/modules/tenant-runner test -no-color

# Operational alarm semantics are exercised against a mock provider as well.
terraform -chdir=infra/terraform/modules/operational-observability init -backend=false -input=false
terraform -chdir=infra/terraform/modules/operational-observability test -no-color

# Separate-product CI cannot grant itself IAM or platform access.
terraform -chdir=infra/terraform/modules/duckfleet-ci init -backend=false -input=false
terraform -chdir=infra/terraform/modules/duckfleet-ci test -no-color
