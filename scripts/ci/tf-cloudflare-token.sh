#!/usr/bin/env bash
# Export CLOUDFLARE_API_TOKEN for whichever apply leg needs one.
#
# TWO STACKS NEED A CLOUDFLARE CREDENTIAL AND THEY NEED DIFFERENT TOKENS.
# `envs/core` creates an R2 bucket, which wants `Workers R2 Storage: Edit` on
# the ACCOUNT. `envs/prod` writes DNS records, which wants `Zone DNS: Edit` on
# ONE zone. Measured 2026-11-08: the zone-scoped token returns `9109` on
# `/accounts/<id>` and `10000` on `/accounts/<id>/r2/buckets`, so it genuinely
# cannot do core's work — a single shared token would have to carry both scopes
# and be strictly broader than either leg needs. So this script picks a SOURCE
# per leg rather than handing out one token:
#
#   CF_TOKEN_FROM_ENVIRONMENT set  -> use it. That is the `infra-apply-core`
#                                     Environment secret, which resolves only on
#                                     the core leg (the environment is
#                                     `protected_branches: true`, so a pull
#                                     request cannot read it even by editing
#                                     the workflow).
#   otherwise                      -> read Secrets Manager `$SECRET_ID`, with
#                                     the caller's own AWS credentials. On the
#                                     prod leg that is `er-ci-tf-apply-prod`,
#                                     after the §14.2 hop.
#
# WHY THE PROD TOKEN IS NOT ALSO A GITHUB SECRET. `infra.yml` otherwise reads no
# stored secret — every AWS identity is a federated OIDC role — and the PLAN
# tier's role is assumable from any pull-request branch, which can edit the
# workflow it runs under. Keeping prod's token in Secrets Manager means there is
# nothing for that tier to reach even in principle. Core's is an Environment
# secret instead because Cloudflare account-scoped tokens have no AWS-side
# equivalent in that flow and the environment scoping bounds it; the workflow
# header argues that trade.
#
# IT MUST BE CALLED FROM A STEP, NOT SET AS A JOB-LEVEL `env:`. A job- or
# step-level `env:` takes precedence over `$GITHUB_ENV`, so a job-level
# `CLOUDFLARE_API_TOKEN` — even an empty one — silently shadows whatever this
# writes. That is not hypothetical: it is how the first prod edge apply failed,
# with Cloudflare `9106` (no authorization header) while this script reported
# success on the line above it.
#
# WHY NOT TF_VAR_. A Terraform variable is written into the saved plan file, and
# the apply tier plans to `tfplan` then applies that file. The Cloudflare
# provider reads `CLOUDFLARE_API_TOKEN` from the environment natively, so the
# token never enters anything Terraform persists. `envs/{core,dev,prod}`
# all declare `provider "cloudflare" {}` for this reason.
#
# Usage: bash scripts/ci/tf-cloudflare-token.sh
# Env:   CF_TOKEN_FROM_ENVIRONMENT  (optional; wins when non-empty)
#        SECRET_ID                  (default er/shared/cloudflare)
# Exit:  0 exported; 1 no usable token from either source

set -euo pipefail

SECRET_ID="${SECRET_ID:-er/shared/cloudflare}"

if [ -z "${GITHUB_ENV:-}" ]; then
	echo "tf-cloudflare-token: GITHUB_ENV is unset — this script only makes sense inside a GitHub Actions job" >&2
	exit 1
fi

token=""
source=""

# Trimmed, because a token pasted into a secret with a trailing newline would
# be sent as part of the Authorization header and fail as though it were absent.
if [ -n "${CF_TOKEN_FROM_ENVIRONMENT:-}" ]; then
	token="$(printf '%s' "$CF_TOKEN_FROM_ENVIRONMENT" | tr -d ' \t\r\n')"
	source="the job's Environment secret"
else
	# `|| true` deliberately absent: a failure here must stop the apply. An
	# apply that proceeded without the token would plan Cloudflare changes it
	# cannot make and fail later with an authentication error that reads like a
	# bad token rather than a missing one.
	payload="$(aws secretsmanager get-secret-value \
		--secret-id "$SECRET_ID" \
		--query SecretString \
		--output text)"

	token="$(printf '%s' "$payload" | jq -r '.CF_API_TOKEN // empty' | tr -d ' \t\r\n')"
	source="Secrets Manager $SECRET_ID"

	if [ -z "$token" ]; then
		echo "tf-cloudflare-token: $SECRET_ID has no non-empty CF_API_TOKEN key" >&2
		# Name the keys that ARE there, so a renamed key is one line to
		# diagnose rather than a guess. Keys only — never values.
		printf '%s' "$payload" | jq -r 'keys | "  keys present: \(join(", "))"' >&2 || true
		exit 1
	fi
fi

if [ -z "$token" ]; then
	echo "tf-cloudflare-token: no token from either source" >&2
	exit 1
fi

# Mask before export. Without this, any later step that echoed the environment
# would print the token into a public repository's build log.
echo "::add-mask::$token"
echo "CLOUDFLARE_API_TOKEN=$token" >>"$GITHUB_ENV"

echo "tf-cloudflare-token: exported CLOUDFLARE_API_TOKEN from $source (${#token} chars)"
