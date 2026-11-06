#!/usr/bin/env bash
# drift.yml's summary step: the verdict and the refresh-only plan, into the
# run summary, so a red job explains itself there rather than only in the log.
#
# Inputs (env): STACK_KEY, GITHUB_STEP_SUMMARY. Reads driftout/.
set -uo pipefail

{
  echo "### drift — ${STACK_KEY}"
  echo ""
  if [ -s driftout/verdict.txt ]; then
    echo '```'
    cat driftout/verdict.txt
    echo '```'
  else
    echo "_The refresh did not complete; this is not a drift result._"
  fi
  echo ""
  echo "<details><summary>refresh-only plan</summary>"
  echo ""
  echo '```terraform'
  tail -c 40000 driftout/out.txt 2>/dev/null || true
  echo '```'
  echo ""
  echo "</details>"
} >> "$GITHUB_STEP_SUMMARY"
