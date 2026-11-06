#!/usr/bin/env bash
# Step 4a of merge-bus.yml: red, or never concluded — report and stop.
#
# The bus PR is left OPEN on purpose. Its branch is the only place the failing
# combination exists, so closing it would delete the evidence before anyone
# had read the logs.
#
# Inputs (env): BUS, RIDING, VERDICT, FAILED, HELD, RUN_URL, GH_TOKEN.
# Always exits 1.
set -euo pipefail

# `starved` is NOT a verdict about the batch, so it is worded and
# labelled differently. Calling it a red batch would send someone
# debugging a diff that was never compiled.
case "$VERDICT" in
  red)
    headline="went **red** on: ${FAILED}"
    blame="batch" ;;
  starved)
    headline="never ran — GitHub allocated no runner for any job (an Actions minutes or spending limit on the account, not a problem with any PR in the batch)"
    blame="account" ;;
  held)
    headline="is waiting for a human to approve its workflow runs (${HELD}). A bot-authored pull request does not run CI unattended, and this token could not release them — press \"Approve and run\" on the bus PR, then re-dispatch"
    blame="account" ;;
  *)
    headline="did not conclude within the hour the bus waits"
    blame="batch" ;;
esac

if [ "$blame" = "batch" ]; then
  gh pr edit "$BUS" --add-label bus-failed || echo "::warning::could not label #$BUS"
fi
gh pr comment "$BUS" --body "The batch ${headline}. Nothing was merged. Left open so the combination stays inspectable — [run log](${RUN_URL})." || true

# Only tell the members something happened to them when something
# did. During a runner outage every PR in the repository is equally
# stuck, and a comment on each would be noise about nothing.
if [ "$blame" = "batch" ]; then
  for n in $RIDING; do
    gh pr comment "$n" --body "Held: the merge-bus batch in #${BUS} ${headline}, so nothing in that batch merged. This PR is untouched — see the [run log](${RUN_URL}). It may or may not be the cause; the batch was not bisected." || true
  done
fi

echo "::error::batch ${headline}; nothing merged"
exit 1
