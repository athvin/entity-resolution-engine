#!/usr/bin/env bash
# Write the deploy summary: what shipped, and whether it came from THIS commit.
#
# The summary is the artifact someone reads a week later when asking "was that
# change actually out?". A digest alone does not answer it; whether it came
# from this commit does.
#
# Inputs (env): DEPLOY_SHA, API_DIGEST, WEB_DIGEST, API_REUSED, WEB_REUSED;
# optional DEPLOY_LABEL (defaults to the dev `ci` namespace's).
set -euo pipefail

{
	echo "### deploy — ${DEPLOY_LABEL:-dev (\`ci\` namespace)}"
	echo ""
	echo "| image | digest | from this commit? |"
	echo "|---|---|---|"
	for pair in "er-api:$API_REUSED:$API_DIGEST" \
		"er-web:$WEB_REUSED:$WEB_DIGEST"; do
		name="${pair%%:*}"
		rest="${pair#*:}"
		reused="${rest%%:*}"
		digest="${rest#*:}"
		if [ "$reused" = "1" ]; then
			echo "| \`$name\` | \`${digest:0:19}…\` | **no — reused the newest digest in ECR** |"
		else
			echo "| \`$name\` | \`${digest:0:19}…\` | yes |"
		fi
	done
	echo ""
	echo "Commit: \`$DEPLOY_SHA\`"
} >> "$GITHUB_STEP_SUMMARY"
