#!/usr/bin/env bash
# Export CLOUDFLARE_API_TOKEN for the prod apply tier, from Secrets Manager.
#
# WHY THE SECRET LIVES IN AWS AND NOT IN A GITHUB SECRET. `infra.yml` reads no
# stored repository secret — every identity it uses is a federated OIDC role —
# and that property is worth keeping for one specific reason: the PLAN tier's
# role (`er-ci-tf-plan`) is assumable from any pull-request branch, and a pull
# request can edit the workflow it runs under. A repository secret available to
# that tier would be exfiltratable by any contributor who can open a PR.
#
# So the token sits in Secrets Manager and is fetched here, in the apply job,
# AFTER the chain into `er-ci-tf-apply-prod` — which already carries
# AdministratorAccess (envs/core/ci-terraform.tf), so no new IAM grant was
# needed and the apply-before-merge ordering trap (#358, #350) does not apply.
#
# The plan tier calls this script not at all, and needs nothing: a
# `-refresh=false` plan over Cloudflare resources configures the provider
# without validating credentials. Verified 2026-11-08 — a credential-free plan
# of a `cloudflare_dns_record` returned `Plan: 1 to add`. The constraint that
# keeps it true is in envs/prod/edge.tf's header: no Cloudflare `data` sources,
# because a data source IS read at plan time.
#
# WHY NOT TF_VAR_. A Terraform variable would be written into the saved plan
# file, and the apply tier plans to `tfplan` then applies that file. The
# Cloudflare provider reads `CLOUDFLARE_API_TOKEN` from the environment
# natively, so the token never enters Terraform's persisted state at all. This
# is the convention envs/dev/versions.tf already documents.
#
# Usage: bash scripts/ci/tf-cloudflare-token.sh
# Env:   SECRET_ID  (default er/shared/cloudflare)
# Exit:  0 exported; 1 the secret or its key is missing

set -euo pipefail

SECRET_ID="${SECRET_ID:-er/shared/cloudflare}"

if [ -z "${GITHUB_ENV:-}" ]; then
	echo "tf-cloudflare-token: GITHUB_ENV is unset — this script only makes sense inside a GitHub Actions job" >&2
	exit 1
fi

# `|| true` deliberately absent: a failure here must stop the apply. An apply
# that proceeded without the token would plan Cloudflare changes it cannot
# make, and fail later with an authentication error that reads like a bad token
# rather than a missing one.
payload="$(aws secretsmanager get-secret-value \
	--secret-id "$SECRET_ID" \
	--query SecretString \
	--output text)"

token="$(printf '%s' "$payload" | jq -r '.CF_API_TOKEN // empty')"

if [ -z "$token" ]; then
	echo "tf-cloudflare-token: $SECRET_ID has no non-empty CF_API_TOKEN key" >&2
	# Name the keys that ARE there, so a renamed key is one line to diagnose
	# rather than a guess. Keys only — never values.
	printf '%s' "$payload" | jq -r 'keys | "  keys present: \(join(", "))"' >&2 || true
	exit 1
fi

# Mask before export. Without this, any later step that echoed the environment
# would print the token into a public repository's build log.
echo "::add-mask::$token"
echo "CLOUDFLARE_API_TOKEN=$token" >>"$GITHUB_ENV"

echo "tf-cloudflare-token: exported CLOUDFLARE_API_TOKEN from $SECRET_ID (${#token} chars)"
