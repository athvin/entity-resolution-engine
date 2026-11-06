#!/usr/bin/env bash
# Step 3 of merge-bus.yml: wait for the combination's verdict.
#
# WAITING FOR `CLEAN` IS WHAT MAKES "NO CHECKS YET" SAFE. A freshly opened PR
# has reported nothing, so a predicate phrased as "nothing is failing" would
# call it green within seconds of creation. CLEAN is reachable only once every
# required context has reported success, so the wait cannot end early — and an
# explicit failed conclusion short-circuits it, so a genuine red does not cost
# the whole timeout either.
#
# Inputs (env): BUS, BUS_SHA, GITHUB_REPOSITORY, GH_TOKEN.
# Outputs ($GITHUB_OUTPUT): verdict, failed, held.
set -euo pipefail

# ACTION_REQUIRED IS NOT A BATCH FAILURE, and listing it alongside the
# real ones was wrong. A bot-authored pull request's workflow runs are
# created `conclusion: action_required` with ZERO jobs, waiting for a
# human to press "Approve and run" — observed on bus PR #585, all
# three runs held. Calling that "the batch went red" accuses the
# members of a failure nothing executed, and it is indistinguishable
# from starvation by step count, because both are zero.
failure_states='FAILURE|TIMED_OUT|CANCELLED|STARTUP_FAILURE|ERROR'
deadline=$(( SECONDS + 60 * 60 ))
verdict="timeout"; failed=""; held=""

while [ "$SECONDS" -lt "$deadline" ]; do
  state="$(gh pr view "$BUS" --json mergeStateStatus --jq .mergeStateStatus 2>/dev/null || echo UNREADABLE)"
  if [ "$state" = "CLEAN" ]; then
    verdict="green"; break
  fi

  # NOT fatal if it cannot be read. This assignment used to abort the
  # whole step under `set -e` the moment the rollup was inaccessible,
  # which is how the first successful batch died with the bus PR left
  # open and no report. An unreadable rollup means "keep waiting".
  rollup=""
  if ! rollup="$(gh pr view "$BUS" --json statusCheckRollup --jq \
       '[.statusCheckRollup[]? | {n: (.name // .context), c: ((.conclusion // .state) // "")}] | map("\(.n)\t\(.c)") | join("\n")' 2>/dev/null)"; then
    echo "  $(date -u +%H:%M:%S) ${state} — rollup unreadable (needs \`checks: read\`); waiting"
    sleep 30; continue
  fi

  failed="$(printf '%s\n' "$rollup" | awk -F'\t' -v re="$failure_states" \
    'NF==2 && $2 ~ re {print $1}' | sort -u | paste -sd, - | sed 's/,/, /g')"
  if [ -n "$failed" ]; then verdict="red"; break; fi

  # HELD-NESS COMES FROM THE ACTIONS API, NOT THE PR ROLLUP, and that
  # is not a stylistic choice: a held run has ZERO jobs, so it creates
  # ZERO check-runs, so `statusCheckRollup` is EMPTY. Reading
  # ACTION_REQUIRED out of the rollup — as the first version did —
  # can therefore never match.
  #
  # Measured on bus PR #601: three runs `conclusion:
  # action_required`, rollup `entries=0`; approving them took it
  # straight to `entries=29`. Left alone the bus would have polled its
  # whole hour, timed out, then had the starvation classifier see zero
  # executed steps and report an Actions spending limit — the wrong
  # cause, loudly, which is the failure mode that classifier exists to
  # prevent.
  #
  # The run names stand in for context names here; no check-run exists
  # to name, which is the whole point.
  held="$(gh api "repos/${GITHUB_REPOSITORY}/actions/runs?head_sha=${BUS_SHA}&per_page=20" \
            --jq '[.workflow_runs[] | select(.conclusion == "action_required") | .name] | unique | join(", ")' \
          2>/dev/null || echo "")"
  case "$held" in *$'\n'*) held="" ;; esac
  if [ -n "$held" ]; then
    # TRY TO RELEASE THEM ONCE, then fall back to asking a human.
    #
    # Whether a `GITHUB_TOKEN` may approve runs it caused is the open
    # question this attempt answers; if GitHub refuses, the bus says
    # so and stops rather than pretending the batch failed. Approving
    # runs no new untrusted code — the bus PR is a merge of PRs whose
    # own CI already ran this exact content — so the substance of the
    # gate is unaffected either way.
    if [ "${approved:-0}" = "1" ]; then
      verdict="held"; break
    fi
    approved=1
    echo "  $(date -u +%H:%M:%S) runs awaiting approval (${held}); attempting to release them"
    any=0
    for rid in $(gh api "repos/${GITHUB_REPOSITORY}/actions/runs?head_sha=${BUS_SHA}&per_page=20" \
                   --jq '.workflow_runs[] | select(.conclusion == "action_required") | .id' 2>/dev/null); do
      case "$rid" in ''|*[!0-9]*) continue ;; esac
      if gh api -X POST "repos/${GITHUB_REPOSITORY}/actions/runs/${rid}/approve" >/dev/null 2>&1; then
        any=1; echo "    approved run ${rid}"
      else
        echo "::warning::could not approve run ${rid}"
      fi
    done
    if [ "$any" = "0" ]; then
      echo "::warning::no run could be approved by this token"
      verdict="held"; break
    fi
    sleep 30; continue
  fi

  echo "  $(date -u +%H:%M:%S) ${state} — waiting"
  sleep 30
done

# DISTINGUISH "THE BATCH IS BAD" FROM "NO RUNNER WAS ALLOCATED".
# Both present as `failure`, and blaming the batch for the second is
# the misattribution the pre-flight above describes — it just arrives
# from the other direction, when capacity runs out mid-validation.
# Authoritative here because it reads the bus PR's OWN runs.
# `held` already carries its own explanation, and is byte-identical
# to starvation by step count (both zero), so it must not be relabelled.
if [ "$verdict" != "green" ] && [ "$verdict" != "held" ]; then
  head_sha="$(gh pr view "$BUS" --json headRefOid --jq .headRefOid)"
  # `read_ok` matters more than `executed`. If the Actions API cannot
  # be read, `executed` stays 0 — which would relabel a genuinely RED
  # batch as `starved`, suppressing the bus-failed label and the
  # member comments and telling everyone to go look at their billing.
  # Claiming starvation requires evidence, not the absence of it.
  read_ok=0; executed=0; runs=""
  if runs="$(gh api "repos/${GITHUB_REPOSITORY}/actions/runs?head_sha=${head_sha}&per_page=20" \
               --jq '.workflow_runs[].id' 2>/dev/null)"; then
    for rid in $runs; do
      case "$rid" in ''|*[!0-9]*) continue ;; esac
      steps="$(gh api "repos/${GITHUB_REPOSITORY}/actions/runs/${rid}/jobs?per_page=100" \
                 --jq '[.jobs[].steps | length] | add // 0' 2>/dev/null || echo skip)"
      case "$steps" in ''|*[!0-9]*) continue ;; esac
      read_ok=1
      executed=$((executed + steps))
    done
  fi
  if [ "$read_ok" -eq 1 ]; then
    echo "steps executed across the bus PR's runs: ${executed}"
    [ "$executed" -eq 0 ] && verdict="starved"
  else
    echo "::warning::could not read the bus PR's run history; reporting as ${verdict} rather than guessing at starvation"
  fi
fi

echo "verdict: $verdict"
{
  echo "verdict=$verdict"
  echo "failed=$failed"
  echo "held=$held"
} >>"$GITHUB_OUTPUT"
