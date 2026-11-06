#!/usr/bin/env bash
# Append a terraform plan or apply to the job summary, fenced and truncated.
#
# infra.yml's plan tier and apply tier each carried this. They differ only in
# the heading, which file holds the plan text, and whether a meta block goes
# above it — so those are the three parameters.
#
# WHAT MAY REACH THE SUMMARY. Only the `terraform show -no-color` HUMAN form,
# which redacts sensitive values to `(sensitive value)`. The saved plan file
# and `show -json` both carry attribute values in cleartext and must never be
# uploaded or printed; see the comments on the steps that produce them. This
# script is given the redacted text file and never the other two.
#
# For the plan tier this is also the UNTRUNCATED copy relative to the PR
# comment — the comment links here when a plan is too large to inline — which
# is why the 40000-byte tail is much larger than the comment's budget.
#
# A MISSING FILE SAYS SO rather than rendering an empty fence. The plan tier's
# inline copy swallowed it with `|| true`, so a tier that produced no plan at
# all looked identical to one whose plan was genuinely empty; the apply tier's
# copy already printed the message, and that is the behaviour kept here.
#
# Inputs (env): TITLE, PLAN (path to the redacted text), META (optional path,
# indented four spaces above the fence).
# Run: TITLE='terraform plan — core' PLAN=planout/plan.txt META=planout/meta.txt \
#        bash scripts/ci/tf-step-summary.sh
set -euo pipefail

: "${TITLE:?TITLE must be set}"
: "${PLAN:?PLAN must name the redacted plan text file}"

{
	echo "### $TITLE"
	echo ""
	if [ -n "${META:-}" ]; then
		# Four-space indent renders it as a code block without a fence, so
		# the plan's own fence below stays the only one.
		sed 's/^/    /' "$META" 2>/dev/null || true
		echo ""
	fi
	echo '```terraform'
	tail -c 40000 "$PLAN" 2>/dev/null || echo "(no plan was produced)"
	echo '```'
} >> "$GITHUB_STEP_SUMMARY"
