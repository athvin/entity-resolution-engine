#!/usr/bin/env bash
# Step 1 of merge-bus.yml: choose the batch — who is riding?
#
# ELIGIBILITY ASKS GITHUB; IT DOES NOT RESTATE THE REQUIRED SET. With the
# review requirement at zero, a PR whose required checks are all satisfied is
# reported by GitHub as `mergeStateStatus: CLEAN`, so that one field IS the
# verdict. Enumerating the required contexts here instead would duplicate
# branch protection into a place it can drift from, and would need an
# admin-scoped protection read that `GITHUB_TOKEN` does not have.
#
# Every rejection prints its reason. A gate that cannot say why it excluded
# something is a gate that passes by examining nothing.
#
# Inputs (env): MAX_BATCH_INPUT, MAX_BATCH_VAR, GITHUB_REPOSITORY, GH_TOKEN.
# Outputs ($GITHUB_OUTPUT): members.
set -euo pipefail

limit="${MAX_BATCH_INPUT:-}"
[ -n "$limit" ] || limit="${MAX_BATCH_VAR:-}"
[ -n "$limit" ] || limit=4
case "$limit" in
  ''|*[!0-9]*) echo "::error::max_batch must be a positive integer, got '$limit'"; exit 1 ;;
esac
[ "$limit" -ge 1 ] || { echo "::error::max_batch must be at least 1, got '$limit'"; exit 1; }
echo "batch limit: $limit"

# Opt-in by label, oldest first. Ascending PR number makes the queue
# predictable and stops a long-waiting PR being starved by newer ones.
candidates=()
while IFS= read -r n; do
  [ -n "$n" ] && candidates+=("$n")
done < <(gh pr list --state open --base main --label merge-bus \
           --limit 100 --json number --jq '.[].number' | sort -n)

if [ "${#candidates[@]}" -eq 0 ]; then
  echo "no open PR carries the merge-bus label; nothing to do"
  echo "members=" >>"$GITHUB_OUTPUT"
  exit 0
fi

# How many of this commit's `detect` jobs FAILED — "0" when none did,
# and "0" as well when the answer cannot be read.
#
# A FAILED API CALL IS NOT DATA, and the first version of this check
# got that wrong in a way worth spelling out, because it reads as
# correct. It was:
#
#   [ "$(gh api ... --paginate --jq '...|length' 2>/dev/null || echo 0)" != "0" ]
#
# `--paginate` emits one jq result per page, and `|| echo 0` appends
# another on failure — so a call that emits page one and THEN errors
# (a secondary rate limit is the obvious candidate, given how many
# calls this loop makes) yields the two-line string "0\n0". That is
# `!= "0"`, so a perfectly healthy PR gets skipped as unverified.
#
# Measured 2026-11-05 on #510: all three detect jobs had completed
# `success` twenty minutes earlier, the PR was CLEAN, and the bus
# skipped it anyway. This is the same defect the pre-flight above was
# already fixed for, reintroduced here one commit later.
#
# So: read the status explicitly, require a single bare number, and
# FAIL OPEN on anything else — consistent with the pre-flight.
# `mergeStateStatus == CLEAN` is still required, so this check is a
# safety net rather than the gate, and a rate limit must not be able
# to block every merge. The warning is loud instead.
#
# NOT `--paginate`, and not whitespace-collapsing either: `"0\n0"`
# with the newline stripped becomes `"00"`, which is still numeric and
# still `!= "0"` — the same bug wearing a different hat. One page of
# 100 is fetched and `total_count` is compared against it, so a commit
# with more check-runs than that says so rather than quietly answering
# from a prefix.
detect_failures() {
  local oid="$1" raw fails total
  [ -n "$oid" ] || { echo "::warning::no head oid for the detect check" >&2; echo 0; return; }
  if ! raw="$(gh api "repos/${GITHUB_REPOSITORY}/commits/${oid}/check-runs?per_page=100" \
                --jq '"\([.check_runs[] | select(.name | startswith("detect")) | select(.conclusion == "failure")] | length) \(.total_count) \(.check_runs | length)"' \
              2>/dev/null)"; then
    echo "::warning::could not read check-runs for ${oid}; not treating that as a detect failure" >&2
    echo 0; return
  fi
  # Exactly three space-separated integers on one line, or we do not
  # understand the answer and must not act on it.
  case "$raw" in
    *$'\n'*|'')
      echo "::warning::unexpected check-runs response for ${oid} (${raw}); ignoring" >&2
      echo 0; return ;;
  esac
  set -- $raw
  [ "$#" -eq 3 ] || { echo "::warning::unparseable check-runs response for ${oid}: '${raw}'" >&2; echo 0; return; }
  fails="$1"; total="$2"; fetched="$3"
  for v in "$fails" "$total" "$fetched"; do
    case "$v" in ''|*[!0-9]*) echo "::warning::non-numeric check-runs field for ${oid}: '${raw}'" >&2; echo 0; return ;; esac
  done
  if [ "$total" -gt "$fetched" ]; then
    echo "::warning::${oid} has ${total} check-runs but only ${fetched} were read; the detect answer may be partial" >&2
  fi
  echo "$fails"
}

chosen=()
printf '\n%-6s  %-9s  %s\n' "PR" "VERDICT" "REASON"
printf -- '------  ---------  ------------------------------------------\n'

for n in "${candidates[@]}"; do
  # `gh pr list` reports mergeStateStatus UNKNOWN for a PR whose
  # mergeability GitHub has not computed, and LISTING DOES NOT TRIGGER
  # THE COMPUTATION — `gh pr view` does. Measured 2026-11-03: every PR
  # in a fresh `list` read UNKNOWN, and the same PRs read
  # DIRTY/BLOCKED through `view`. So the authoritative read is per-PR,
  # and UNKNOWN is retried rather than believed in either direction.
  state=""; draft=""; base=""; head=""; oid=""
  for _ in 1 2 3 4 5; do
    IFS='|' read -r state draft base head oid < <(gh pr view "$n" \
      --json mergeStateStatus,isDraft,baseRefName,headRefName,headRefOid \
      --jq '[.mergeStateStatus, (.isDraft|tostring), .baseRefName, .headRefName, .headRefOid] | join("|")')
    [ "$state" = "UNKNOWN" ] || break
    sleep 5
  done

  reason=""
  if [ "$draft" != "false" ]; then
    reason="draft"
  elif [ "$base" != "main" ]; then
    # A stacked PR. Its diff is against its parent, so putting it on a
    # batch branch cut from `main` would stage its parent's changes
    # under its name.
    reason="stacked on '$base', not main"
  elif [ "${head#merge-bus/}" != "$head" ]; then
    # Without this the bus batches its own bus PR on the next run.
    reason="is itself a bus branch"
  elif [ "$(gh pr view "$n" --json files \
              --jq '[.files[].path | select(startswith(".github/workflows/"))] | length')" != "0" ]; then
    # A HARD PLATFORM LIMIT, NOT A POLICY CHOICE. GitHub refuses a
    # push from an App-authenticated token that creates or updates
    # anything under `.github/workflows/`:
    #
    #   ! [remote rejected] refusing to allow a GitHub App to create
    #     or update workflow `.github/workflows/rust.yml` without
    #     `workflows` permission
    #
    # And `workflows` is NOT a grantable `GITHUB_TOKEN` scope —
    # the permissions block accepts only actions, attestations,
    # checks, contents, deployments, discussions, id-token, issues,
    # packages, pages, pull-requests, repository-projects,
    # security-events and statuses. So no `permissions:` line can fix
    # this; only a PAT carrying the `workflow` scope could, which
    # would hand a long-lived credential push rights over CI itself.
    #
    # Observed on the second live run: everything worked — pre-flight,
    # batch merge, tree — and the push was rejected. Nothing was left
    # behind, because the rejection happens before the branch exists.
    #
    # Such a PR must be merged by hand. Said plainly here so nobody
    # reads the skip as the bus being broken.
    reason="touches .github/workflows/ — GITHUB_TOKEN may not push workflow changes, so merge it by hand"
  elif [ "$(detect_failures "$oid")" != "0" ]; then
    # A GREEN THAT VERIFIED NOTHING. Every expensive job is gated on
    # `needs.detect.outputs.*`, so when a `detect` job FAILS its
    # dependents report `skipped` — and branch protection accepts
    # `skipped` as satisfied. The result is a PR whose twelve required
    # contexts are all "satisfied" with not one of them having run.
    #
    # Observed on #473 (2026-11-05): two Rust files changed, all three
    # detect jobs failed in the Actions-minutes outage, all 12 required
    # contexts `skipped`, `mergeStateStatus` CLEAN. A human or this bus
    # could have merged it with nothing compiled or tested.
    #
    # `skipped` is only trustworthy when the job that decided to skip
    # SUCCEEDED. This is the same "false skip is the worst failure mode
    # available" that the `grep -c`-not-`-q` rule exists for, reached
    # by a different route: there the detect job lies, here it dies.
    #
    # Not fixable by requiring the detect contexts in branch
    # protection — they are matrix-free but there are three of them
    # with two distinct names, and a required context that legitimately
    # does not report would block every PR. Checking it here is cheap
    # and precise.
    reason="its detect jobs failed, so every gate reported skipped without running — re-run CI before batching"
  else
    case "$state" in
      CLEAN) ;;
      DIRTY)
        # A conflicted PR runs ZERO workflows and reports "no checks
        # reported". Any predicate phrased as "nothing is failing"
        # reads that as green, so DIRTY is rejected by name.
        reason="conflicts with main (runs no CI, so reports no checks)" ;;
      BLOCKED)
        reason="a required check is missing, failing or still pending" ;;
      UNSTABLE)
        # Deliberately conservative, and note UNSTABLE means red OR
        # STILL PENDING on a non-required check — not red alone. Both
        # occur here: a PR red only on the non-required `semgrep
        # (static analysis)` context, which a human may merge, and a
        # PR merely waiting on one (observed on #484, queued on
        # `record pass marker`, which went CLEAN by itself minutes
        # later).
        #
        # The distinction matters for whoever reads the table, not for
        # the bus: reporting "red" when a check is only queued sends
        # someone hunting a failure that does not exist. Either way
        # the bus takes only what GitHub calls fully clean, so a
        # pending one simply rides the next round. Widen this with an
        # explicit input rather than by loosening the predicate.
        reason="a non-required check is red or still pending (UNSTABLE)" ;;
      BEHIND)
        reason="behind main and protection demands an update" ;;
      UNKNOWN)
        reason="GitHub would not compute mergeability after 5 attempts" ;;
      *)
        reason="unrecognised mergeStateStatus '$state'" ;;
    esac
  fi

  if [ -n "$reason" ]; then
    printf '%-6s  %-9s  %s\n' "#$n" "skip" "$reason"
  elif [ "${#chosen[@]}" -ge "$limit" ]; then
    printf '%-6s  %-9s  %s\n' "#$n" "defer" "ready, but the batch is full at $limit"
  else
    chosen+=("$n")
    printf '%-6s  %-9s  %s\n' "#$n" "RIDE" "clean"
  fi
done

echo ""
if [ "${#chosen[@]}" -eq 0 ]; then
  echo "no eligible PR this round"
  echo "members=" >>"$GITHUB_OUTPUT"
  exit 0
fi
echo "batch: ${chosen[*]}"
echo "members=${chosen[*]}" >>"$GITHUB_OUTPUT"
