#!/usr/bin/env bash
# The implementation loop driver.
#
# Runs `claude -p "/implementing-er-tickets"` in a while-loop, ONE FRESH CONTEXT
# PER TICKET, until the board reports no READY tickets. Safe to leave unattended:
# it stops on a stalled board, an iteration cap, a spend cap, a stop file, or two
# consecutive environment failures, and it independently re-verifies every ticket
# the agent claims to have finished.
#
# Usage:
#   scripts/run-loop.sh                          # bounded run: 25 iterations, $25
#   scripts/run-loop.sh --max-iterations 3       # bounded run (use this first)
#   scripts/run-loop.sh --ticket ER-013          # a single named ticket, then stop
#   scripts/run-loop.sh --dry-run                # preflight only
#
# Stop it with: touch .loop/STOP     (finishes the current iteration, then exits)
#
# Exit codes:
#   0 board complete   3 preflight failed   6 spend cap
#   1 iteration failed 4 board stalled      7 repeated environment failure
#   2 usage            5 iteration cap      8 protected artifact modified
#                                           9 spec changed mid-run
#                                          10 no progress for two iterations
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 3

BOARD="$REPO_ROOT/scripts/board.py"
GATES="$REPO_ROOT/scripts/gates.sh"
LOOP_DIR="$REPO_ROOT/.loop"
STOP_FILE="$LOOP_DIR/STOP"
LOCK_FILE="$LOOP_DIR/loop.lock"

MAX_ITERATIONS=25           # bounded by default; pass 0 to run until the board is clear
MAX_TURNS=200
MAX_BUDGET_USD=10
TOTAL_BUDGET_USD=25         # bounded by default; pass 0 to remove the cap
ONLY_TICKET=""
DRY_RUN=0
QUARANTINE=1
MODEL=""

# Long gates (a cold Compose integration run) exceed the Bash tool's 2-minute
# default and its 10-minute ceiling. Raise both for the child process.
export BASH_DEFAULT_TIMEOUT_MS="${BASH_DEFAULT_TIMEOUT_MS:-600000}"
export BASH_MAX_TIMEOUT_MS="${BASH_MAX_TIMEOUT_MS:-3600000}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --max-iterations) MAX_ITERATIONS="${2:-0}"; shift 2 ;;
    --max-turns) MAX_TURNS="${2:-200}"; shift 2 ;;
    --max-budget-usd) MAX_BUDGET_USD="${2:-10}"; shift 2 ;;
    --total-budget-usd) TOTAL_BUDGET_USD="${2:-0}"; shift 2 ;;
    --ticket) ONLY_TICKET="${2:-}"; shift 2 ;;
    --model) MODEL="${2:-}"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --no-quarantine) QUARANTINE=0; shift ;;
    -h|--help) sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "run-loop.sh: unknown argument '$1'" >&2; exit 2 ;;
  esac
done

mkdir -p "$LOOP_DIR" "$LOOP_DIR/runs" "$LOOP_DIR/receipts" "$LOOP_DIR/logs"

RUN_ID="$(python3 -c 'import uuid;print(str(uuid.uuid4()))')"
RUN_DIR="$LOOP_DIR/runs/$RUN_ID"
mkdir -p "$RUN_DIR"
SUMMARY="$RUN_DIR/summary.tsv"
printf 'iteration\tticket\toutcome\tturns\tcost_usd\tseconds\n' > "$SUMMARY"

log() { printf '[loop %s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
die() { log "FATAL: $*"; exit "${2:-1}"; }

# ---------------------------------------------------------------------------
# single-writer lock. flock(1) does not exist on macOS, so use fcntl via python.
# ---------------------------------------------------------------------------
python3 - "$LOCK_FILE" <<'PY' &
import fcntl, os, signal, sys, time
path = sys.argv[1]
handle = open(path, "w")
try:
    fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
except OSError:
    sys.exit(1)
handle.write(str(os.getppid()) + "\n")
handle.flush()
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    time.sleep(3600)
PY
LOCK_PID=$!
sleep 0.4
if ! kill -0 "$LOCK_PID" 2>/dev/null; then
  die "another run-loop.sh holds $LOCK_FILE" 3
fi
cleanup() { kill "$LOCK_PID" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
# preflight
# ---------------------------------------------------------------------------
log "preflight (run $RUN_ID)"

for tool in claude python3 git; do
  command -v "$tool" >/dev/null 2>&1 || die "'$tool' is not on PATH" 3
done

[[ -f "$STOP_FILE" ]] && die "stop file present: rm $STOP_FILE to resume" 3

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[[ "$BRANCH" == "main" ]] || die "must start on main (on '$BRANCH')" 3

DIRTY="$(git status --porcelain)"
if [[ -n "$DIRTY" ]]; then
  log "working tree is dirty:"; echo "$DIRTY"
  die "commit or stash before starting the loop" 3
fi

python3 "$BOARD" validate --strict || die "board validation failed" 3
python3 "$BOARD" audit || die "board and git disagree; run 'board.py release <ID>' or 'audit'" 3

# Any READY ticket needing the full ladder needs a working Docker daemon. Finding
# that out per-ticket would burn one environment block per integration ticket.
NEEDS_DOCKER="$(python3 - <<'PY'
import json, subprocess, sys
sys.path.insert(0, "scripts")
out = subprocess.run([sys.executable, "scripts/board.py", "next", "--json"],
                     capture_output=True, text=True)
try:
    data = json.loads(out.stdout or "{}")
except json.JSONDecodeError:
    data = {}
print("yes" if data.get("gates") == "full" else "no")
PY
)"
if [[ "$NEEDS_DOCKER" == "yes" ]]; then
  docker info >/dev/null 2>&1 || die "next ticket needs the full gate ladder but Docker is not running" 3
  if [[ -f docker/Dockerfile ]]; then
    log "pre-building er-pipeline:ci so no iteration pays the build cost"
    docker build -f docker/Dockerfile -t er-pipeline:ci . >"$RUN_DIR/image-build.log" 2>&1 \
      || log "WARNING: image pre-build failed; see $RUN_DIR/image-build.log"
  fi
fi

SPEC_SHA_AT_START="$(python3 -c '
import hashlib,pathlib
p=pathlib.Path("DesignDoc.md")
print(hashlib.sha256(p.read_bytes()).hexdigest()[:16] if p.exists() else "")')"

# The permission sandbox. Passed INLINE rather than relying on the skill's
# turn-scoped `allowed-tools` grant satisfying `dontAsk` -- that interaction is
# not documented, and if it does not hold every board.py call is denied.
# Paths are absolute and byte-identical to the ones SKILL.md tells Claude to run.
# NO APOSTROPHES BELOW until the PY terminator. bash 3.2 (macOS) mis-parses an odd
# number of ' characters inside a quoted heredoc nested in $( ), and the script
# then dies with 'unexpected EOF while looking for matching'. Verified the hard way.
SETTINGS_JSON="$(python3 - "$REPO_ROOT" <<'PY'
import json, sys
root = sys.argv[1]
allow = [
    f"Bash(python3 {root}/scripts/board.py *)",
    f"Bash(bash {root}/scripts/gates.sh *)",
    # Both spellings. SKILL.md tells the agent to use the absolute path, but agents
    # routinely shorten to the repo-relative form, and a permission rule matches the
    # literal string -- so a relative call matches no absolute rule and dontAsk denies
    # it. That cost a whole run (two iterations, no progress) before it was caught.
    # The cwd is always the repo root, so both spellings name the same script.
    "Bash(python3 scripts/board.py *)",
    "Bash(bash scripts/gates.sh *)",
    "Bash(./scripts/gates.sh *)",
    # Step 6 runs each ticket OWN verify command, and 61 of the 104 tickets have a
    # verify that is not a `uv` call. Derived by enumerating every verify on the
    # board rather than guessing: itest.sh alone appears in 62 of them. Without these
    # the agent cannot execute step 6 for most of the remaining board -- which is how
    # ER-018 burned three attempts. Bare and argument forms both, because several
    # verifies (actionlint.sh, compose_smoke.sh) take no arguments and `*` does not
    # match empty.
    "Bash(bash scripts/ci/itest.sh *)", "Bash(bash scripts/ci/itest.sh)",
    "Bash(bash scripts/ci/bench.sh *)", "Bash(bash scripts/ci/bench.sh)",
    "Bash(bash scripts/ci/actionlint.sh *)", "Bash(bash scripts/ci/actionlint.sh)",
    "Bash(bash scripts/ci/compose_smoke.sh *)", "Bash(bash scripts/ci/compose_smoke.sh)",
    f"Bash(bash {root}/scripts/ci/itest.sh *)",
    f"Bash(bash {root}/scripts/ci/bench.sh *)",
    f"Bash(bash {root}/scripts/ci/actionlint.sh *)",
    f"Bash(bash {root}/scripts/ci/compose_smoke.sh *)",
    "Bash(python3 scripts/lint_spec.py *)", "Bash(python3 scripts/lint_board.py *)",
    f"Bash(python3 {root}/scripts/lint_spec.py *)",
    f"Bash(python3 {root}/scripts/lint_board.py *)",
    "Bash(docker run *)",
    "Bash(uv run pytest *)", "Bash(uv run ruff *)", "Bash(uv run mypy *)",
    "Bash(uv run dbt *)",
    "Bash(uv sync *)", "Bash(uv lock *)", "Bash(uv add *)",
    # `uv --version` is the first thing anyone runs to check the toolchain exists.
    # Without it the agent probes, gets denied, and concludes uv is unavailable --
    # a false `environment` block on a machine where `uv sync --frozen` works fine.
    "Bash(uv --version)", "Bash(command -v *)",
    # ER-003 AC1 (`uv run python -c "import er"`) and AC7 (`uv run python
    # scripts/actionlint.py --version`) cannot be satisfied without this, so the
    # board does not start without it. It IS arbitrary code execution and so makes
    # the Edit/Write deny rules advisory rather than absolute; the backstops are the
    # hygiene gate protected-path diff and this driver own post-iteration
    # `git diff` over the spec, skill and machinery (exit 8), neither of which the
    # agent can reach.
    "Bash(uv run python *)",
    "Bash(docker compose *)", "Bash(docker build *)", "Bash(docker info)",
    "Bash(git status *)", "Bash(git diff *)", "Bash(git log *)", "Bash(git show *)",
    "Bash(git rev-parse *)", "Bash(git switch *)", "Bash(git add *)",
    "Bash(git commit *)", "Bash(git merge --ff-only *)", "Bash(git branch *)",
    "Bash(mkdir -p *)",
    # Reading goes through the Read/Grep/Glob tools, which are path-constrainable.
    # Bash(cat *) / Bash(ls *) / Bash(grep *) are NOT here: shell redirection is not
    # a command separator, so each of them would permit `... > DesignDoc.md`.
    "Read", "Write", "Edit", "Grep", "Glob", "WebFetch",
]
deny = [
    "Bash(git push *)", "Bash(git reset --hard *)", "Bash(git rebase *)",
    "Bash(git clean *)", "Bash(git restore *)", "Bash(git checkout *)",
    "Bash(git branch -D *)", "Bash(gh *)", "Bash(curl *)",
    f"Bash(bash {root}/scripts/run-loop.sh *)",
    # Only Edit(...) rules are consulted by file permission checks; a Write(...) deny
    # rule is silently inert and merely warns. Edit rules cover every file-editing
    # tool, Write included, so these are the real protection.
    "Edit(DesignDoc.md)",
    "Edit(docs/implementation/**)",
    # NOT all of .loop/: the agent must write .loop/change-plan.json and its logs.
    # Only the artifacts that attest to its work are out of reach.
    "Edit(.loop/receipts/**)",
    "Edit(.loop/gate-cache/**)",
    "Edit(.claude/**)",
    "Edit(scripts/board.py)",
    "Edit(scripts/gates.sh)",
    "Edit(scripts/run-loop.sh)",
    "AskUserQuestion",
]
print(json.dumps({"permissions": {"allow": allow, "deny": deny}}))
PY
)"

# Every ticket's verify command must be executable under the sandbox we just built.
# Discovering otherwise costs three attempts and an auto-block per ticket, and it is
# entirely knowable up front: the board declares every verify command, and the allow
# list is right here. ER-018 burned three attempts proving this check was needed.
VERIFY_UNCOVERED="$(printf '%s' "$SETTINGS_JSON" | python3 scripts/check_verify_perms.py 2>&1)"
VERIFY_RC=$?
if [[ $VERIFY_RC -ne 0 ]]; then
  log "PREFLIGHT FAIL: some ticket verify commands are not permitted by the sandbox."
  echo "$VERIFY_UNCOVERED"
  die "add the missing allow rules to run-loop.sh before starting" 3
fi
[[ -n "$VERIFY_UNCOVERED" ]] && echo "$VERIFY_UNCOVERED"

if [[ $DRY_RUN -eq 1 ]]; then
  log "dry run: preflight passed"
  python3 "$BOARD" status
  exit 0
fi

# ---------------------------------------------------------------------------
# the loop
# ---------------------------------------------------------------------------
ITERATION=0
CONSECUTIVE_ENV=0
NO_PROGRESS=0
TOTAL_COST=0
EXIT_CODE=0
SKILL_VERIFIED=0

while :; do
  if [[ -f "$STOP_FILE" ]]; then
    log "stop file present; exiting cleanly"
    break
  fi
  if [[ $MAX_ITERATIONS -gt 0 && $ITERATION -ge $MAX_ITERATIONS ]]; then
    log "iteration cap ($MAX_ITERATIONS) reached"
    EXIT_CODE=5
    break
  fi
  if [[ $TOTAL_BUDGET_USD != 0 ]]; then
    if python3 -c "import sys;sys.exit(0 if float('$TOTAL_COST')>=float('$TOTAL_BUDGET_USD') else 1)"; then
      log "total spend cap (\$$TOTAL_BUDGET_USD) reached at \$$TOTAL_COST"
      EXIT_CODE=6
      break
    fi
  fi

  NEXT_JSON="$(python3 "$BOARD" next --json)"
  NEXT_RC=$?
  if [[ $NEXT_RC -eq 10 ]]; then
    BLOCKED_N="$(python3 -c "
import json,sys
d=json.loads('''$NEXT_JSON''')
print(len(d.get('blocked',[]))+len(d.get('starved',[])))" 2>/dev/null || echo 0)"
    if [[ "$BLOCKED_N" -gt 0 ]]; then
      log "BOARD STALLED: no READY tickets, but $BLOCKED_N blocked/starved"
      echo "$NEXT_JSON"
      EXIT_CODE=4
    else
      log "board complete: no READY tickets and nothing blocked"
      EXIT_CODE=0
    fi
    break
  elif [[ $NEXT_RC -ne 0 ]]; then
    log "board.py next failed (exit $NEXT_RC)"; echo "$NEXT_JSON"
    EXIT_CODE=1
    break
  fi

  TICKET_ID="$(python3 -c "import json;print(json.loads('''$NEXT_JSON''')['id'])")"
  if [[ -n "$ONLY_TICKET" && "$TICKET_ID" != "$ONLY_TICKET" ]]; then
    TICKET_ID="$ONLY_TICKET"
  fi

  # A leftover ticket branch from an abandoned attempt is poison. SKILL.md step 2
  # runs `git switch -c <branch>`; if the branch already exists that fails, and an
  # agent that presses on lands on the STALE tip -- behind main's own claim commit --
  # finishes the work there, and then cannot ff-merge. That is exactly how ER-018
  # burned two iterations. Clear the branch before handing the ticket over: delete it
  # when it is already merged, otherwise rename it so the work is preserved but the
  # name is free.
  TICKET_BRANCH="$(python3 -c "
import json
d=json.loads('''$NEXT_JSON''')
print(d.get('branch') or '')" 2>/dev/null || echo "")"
  if [[ -n "$TICKET_BRANCH" ]] && git show-ref --verify --quiet "refs/heads/$TICKET_BRANCH"; then
    if git merge-base --is-ancestor "$TICKET_BRANCH" HEAD 2>/dev/null; then
      git branch -d "$TICKET_BRANCH" >/dev/null 2>&1 \
        && log "removed already-merged branch $TICKET_BRANCH"
    else
      ABANDONED="abandoned/${TICKET_BRANCH#ticket/}-$(date -u +%Y%m%dT%H%M%SZ)"
      if git branch -m "$TICKET_BRANCH" "$ABANDONED" >/dev/null 2>&1; then
        log "unmerged branch $TICKET_BRANCH preserved as $ABANDONED"
      else
        log "WARNING: could not clear $TICKET_BRANCH; the iteration may collide"
      fi
    fi
  fi

  ITERATION=$((ITERATION + 1))
  PRE_MAIN="$(git rev-parse HEAD)"
  PRE_DONE="$(python3 -c "
import json,subprocess,sys
o=subprocess.run([sys.executable,'scripts/board.py','status','--json'],capture_output=True,text=True)
print(json.loads(o.stdout)['done'])")"
  BLOCKED_BEFORE=$(wc -l < "$LOOP_DIR/blocked.log" 2>/dev/null || echo 0)
  PRE_ATTEMPTS="$(python3 - "$TICKET_ID" <<'PYA' 2>/dev/null || echo 0
import json, subprocess, sys
o = subprocess.run([sys.executable, "scripts/board.py", "show", sys.argv[1], "--json"],
                   capture_output=True, text=True)
try: print(json.loads(o.stdout).get("attempts", 0))
except Exception: print(0)
PYA
)"
  ITER_LOG="$RUN_DIR/iter-$(printf '%03d' "$ITERATION")-$TICKET_ID.jsonl"
  START=$(date +%s)

  log "iteration $ITERATION -> $TICKET_ID"

  PROMPT="/implementing-er-tickets"
  [[ -n "$ONLY_TICKET" ]] && PROMPT="/implementing-er-tickets $ONLY_TICKET"

  # NOTE: --bare is deliberately NOT passed. It skips skill auto-discovery, so
  # /implementing-er-tickets would not resolve at all.
  CLAUDE_ARGS=(
    -p "$PROMPT"
    --output-format stream-json
    --verbose
    --permission-mode dontAsk
    --settings "$SETTINGS_JSON"
    --max-turns "$MAX_TURNS"
    --max-budget-usd "$MAX_BUDGET_USD"
  )
  [[ -n "$MODEL" ]] && CLAUDE_ARGS+=(--model "$MODEL")

  claude "${CLAUDE_ARGS[@]}" > "$ITER_LOG" 2>>"$RUN_DIR/stderr.log"
  CLAUDE_RC=$?
  ELAPSED=$(( $(date +%s) - START ))

  # Classify from the result event, never from the process exit code and never
  # from the model's prose. A pipeline would mask the status anyway.
  RESULT="$(python3 - "$ITER_LOG" <<'PY'
import json, sys
subtype = "unknown"; cost = 0.0; turns = 0; is_error = False; init_ok = False
skills = []
for line in open(sys.argv[1], errors="replace"):
    line = line.strip()
    if not line:
        continue
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        continue
    if event.get("type") == "system" and event.get("subtype") == "init":
        init_ok = True
        skills = event.get("skills") or event.get("slash_commands") or []
    if event.get("type") == "result":
        subtype = event.get("subtype", "unknown")
        cost = float(event.get("total_cost_usd") or 0.0)
        turns = int(event.get("num_turns") or 0)
        is_error = bool(event.get("is_error"))
found = any("implementing-er-tickets" in str(s) for s in skills)
print(json.dumps({"subtype": subtype, "cost": cost, "turns": turns,
                  "is_error": is_error, "init": init_ok, "skill_listed": found}))
PY
)"
  COST="$(python3 -c "import json;print(json.loads('''$RESULT''')['cost'])")"
  TURNS="$(python3 -c "import json;print(json.loads('''$RESULT''')['turns'])")"
  TOTAL_COST="$(python3 -c "print(float('$TOTAL_COST')+float('$COST'))")"

  # One-time assertion that the skill actually resolved. --bare is documented as
  # becoming the -p default, and when it does, skill discovery disappears
  # silently mid-loop with no error the driver would otherwise notice.
  if [[ $SKILL_VERIFIED -eq 0 ]]; then
    SKILL_LISTED="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['skill_listed'])" "$RESULT" 2>/dev/null || echo False)"
    if [[ "$SKILL_LISTED" != "True" ]]; then
      log "FATAL: the skill /implementing-er-tickets did not resolve in the child session."
      log "       The system/init event did not list it. Check that"
      log "       .claude/skills/implementing-er-tickets/SKILL.md exists, and that this claude"
      log "       build still auto-discovers skills under -p (--bare disables discovery and is"
      log "       documented as becoming the -p default)."
      EXIT_CODE=3
      break
    fi
    SKILL_VERIFIED=1
  fi

  # ---- ground truth: the board and git, not the transcript ----
  POST_DONE="$(python3 -c "
import json,subprocess,sys
o=subprocess.run([sys.executable,'scripts/board.py','status','--json'],capture_output=True,text=True)
print(json.loads(o.stdout)['done'])")"
  TICKET_STATUS="$(python3 -c "
import json,subprocess,sys
o=subprocess.run([sys.executable,'scripts/board.py','show','$TICKET_ID','--json'],capture_output=True,text=True)
print(json.loads(o.stdout).get('status','unknown'))" 2>/dev/null || echo unknown)"

  OUTCOME="$TICKET_STATUS"
  [[ $CLAUDE_RC -ne 0 && "$TICKET_STATUS" == "in_progress" ]] && OUTCOME="crashed"

  printf '%d\t%s\t%s\t%s\t%s\t%d\n' \
    "$ITERATION" "$TICKET_ID" "$OUTCOME" "$TURNS" "$COST" "$ELAPSED" >> "$SUMMARY"
  log "iteration $ITERATION: $TICKET_ID -> $OUTCOME (${ELAPSED}s, \$$COST, $TURNS turns)"

  # ---- guard: nothing the loop is forbidden to touch may have changed ----
  if ! git diff --quiet "$PRE_MAIN" HEAD -- \
        DesignDoc.md .claude scripts/board.py scripts/gates.sh scripts/run-loop.sh 2>/dev/null; then
    log "FATAL: this iteration modified a protected artifact (spec, skill, or loop machinery)."
    log "       Nothing further will run. Inspect: git diff $PRE_MAIN HEAD"
    EXIT_CODE=8
    break
  fi

  SPEC_NOW="$(python3 -c '
import hashlib,pathlib
p=pathlib.Path("DesignDoc.md")
print(hashlib.sha256(p.read_bytes()).hexdigest()[:16] if p.exists() else "")')"
  if [[ "$SPEC_NOW" != "$SPEC_SHA_AT_START" ]]; then
    log "FATAL: DesignDoc.md changed mid-run; tickets already done were verified against different wording."
    EXIT_CODE=9
    break
  fi

  # ---- an interrupted ticket must not be left in_progress ----
  if [[ "$TICKET_STATUS" == "in_progress" ]]; then
    log "ticket left in_progress; releasing it so the board is not wedged"
    git stash push -u -m "loop-wip/$TICKET_ID/$RUN_ID" >/dev/null 2>&1 \
      && log "uncommitted work stashed as loop-wip/$TICKET_ID/$RUN_ID (git stash list)"
    git switch main >/dev/null 2>&1 || true
    python3 "$BOARD" release "$TICKET_ID" || true
    OUTCOME="released"
  fi

  # ---- independent re-verification of merged work ----
  # This is the only check in the whole system that the agent does not control.
  if [[ "$TICKET_STATUS" == "done" && "$POST_DONE" -gt "$PRE_DONE" && $QUARANTINE -eq 1 ]]; then
    log "re-running the full gate ladder on merged main (independent of the agent)"
    if ! bash "$GATES" --scope full --no-cache --no-receipt --ticket "$TICKET_ID" \
         > "$RUN_DIR/reverify-$TICKET_ID.log" 2>&1; then
      log "REVERIFY FAILED for $TICKET_ID -- quarantining and rewinding main"
      QBRANCH="loop-quarantine/$TICKET_ID-$(date -u +%Y%m%d%H%M%S)"
      if ! git branch "$QBRANCH" HEAD; then
        log "cannot create $QBRANCH; refusing to rewind main and lose the work"
        EXIT_CODE=1
        break
      fi
      log "quarantined to $QBRANCH; rewinding main to $PRE_MAIN"
      git reset --hard "$PRE_MAIN"
      python3 "$BOARD" block "$TICKET_ID" \
        --class gate_failed \
        --failing-command "bash scripts/gates.sh --scope full --no-cache (driver re-verification on main)" \
        --assertion "The ticket was marked done, but an independent full-ladder run on merged main failed. See $RUN_DIR/reverify-$TICKET_ID.log" \
        --smallest-fix "Inspect branch loop-quarantine/$TICKET_ID, fix the failing gate, then unblock the ticket." \
        --log "$RUN_DIR/reverify-$TICKET_ID.log" || true
      OUTCOME="quarantined"
    fi
  fi

  # ---- environment-failure circuit breaker ----
  # Inspect only what THIS iteration appended. blocked.log is append-only and
  # survives across runs, so reading its tail made a single historical env block
  # fire the breaker on every later iteration, and a success never reset it.
  BLOCKED_AFTER=$(wc -l < "$LOOP_DIR/blocked.log" 2>/dev/null || echo 0)
  NEW_ENV=0
  if [[ "$BLOCKED_AFTER" -gt "$BLOCKED_BEFORE" ]]; then
    if tail -n $((BLOCKED_AFTER - BLOCKED_BEFORE)) "$LOOP_DIR/blocked.log" \
       | grep -q "class=environment"; then
      NEW_ENV=1
    fi
  fi
  if [[ $NEW_ENV -eq 1 ]]; then
    CONSECUTIVE_ENV=$((CONSECUTIVE_ENV + 1))
  else
    CONSECUTIVE_ENV=0
  fi
  if [[ $CONSECUTIVE_ENV -ge 2 ]]; then
    log "two consecutive environment failures; the machine, not the board, is the problem"
    EXIT_CODE=7
    break
  fi

  # ---- no-progress detector ----
  # An iteration that neither completes a ticket nor consumes an attempt achieved
  # nothing; repeating it burns budget in silence.
  POST_ATTEMPTS="$(python3 - "$TICKET_ID" <<'PYA' 2>/dev/null || echo 0
import json, subprocess, sys
o = subprocess.run([sys.executable, "scripts/board.py", "show", sys.argv[1], "--json"],
                   capture_output=True, text=True)
try: print(json.loads(o.stdout).get("attempts", 0))
except Exception: print(0)
PYA
)"
  if [[ "$POST_DONE" -eq "$PRE_DONE" && "$POST_ATTEMPTS" -le "$PRE_ATTEMPTS" ]]; then
    NO_PROGRESS=$((NO_PROGRESS + 1))
    log "no progress this iteration ($NO_PROGRESS in a row): nothing completed, no attempt consumed"
  else
    NO_PROGRESS=0
  fi
  if [[ $NO_PROGRESS -ge 2 ]]; then
    log "two iterations in a row achieved nothing; stopping rather than burning budget"
    EXIT_CODE=10
    break
  fi

  [[ -n "$ONLY_TICKET" ]] && { log "single-ticket mode; stopping"; break; }
done

# ---------------------------------------------------------------------------
# summary
# ---------------------------------------------------------------------------
log "run $RUN_ID finished after $ITERATION iteration(s), \$$TOTAL_COST"
echo
column -t -s "$(printf '\t')" "$SUMMARY" 2>/dev/null || cat "$SUMMARY"
echo
python3 "$BOARD" status
log "per-iteration transcripts: ${RUN_DIR#$REPO_ROOT/}"
exit $EXIT_CODE
