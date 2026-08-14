#!/usr/bin/env bash
# Single source of truth for the repo-wide gate ladder.
#
# This script is EXECUTED, never read into context, and never re-implemented by
# the model. It is also the only writer of gate receipts: `board.py complete`
# refuses a ticket whose receipt was not produced against the committed tree, so
# "run gates, then edit code, then commit" is detectable rather than invisible.
#
# Usage:
#   scripts/gates.sh --ticket ER-013            # scope from the ticket's `gates:` field
#   scripts/gates.sh --scope fast               # lint + types + unit only
#   scripts/gates.sh --scope full               # everything, including Compose integration
#   scripts/gates.sh --scope auto --ticket ER-013
#   scripts/gates.sh --list                     # print the gate commands and exit
#
# Exit codes: 0 all gates passed; 1 a gate failed; 2 usage; 3 precondition.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 3

BOARD="$REPO_ROOT/scripts/board.py"
RECEIPTS_DIR="$REPO_ROOT/.loop/receipts"
LOGS_DIR="$REPO_ROOT/.loop/logs"
CACHE_DIR="$REPO_ROOT/.loop/gate-cache"
PLAN_PATH="$REPO_ROOT/.loop/change-plan.json"
BASE_REF="${GATES_BASE_REF:-main}"

TICKET=""
SCOPE="auto"
LIST_ONLY=0
NO_CACHE=0
# The driver re-runs the ladder on merged main to check the agent's work. That run
# must not overwrite the receipt the agent's own run produced.
NO_RECEIPT=0

# Suppressions a ticket may never introduce to make a gate pass. A ticket that
# legitimately needs one lists it in its own `allowed_suppressions` frontmatter.
SUPPRESSION_PATTERNS='# *type: *ignore|# *noqa|@pytest\.mark\.skip|@pytest\.mark\.xfail|# *pragma: *no cover|\.skip\(|pytest\.xfail\('

usage() {
  sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ticket) TICKET="${2:-}"; shift 2 ;;
    --scope) SCOPE="${2:-}"; shift 2 ;;
    --list) LIST_ONLY=1; shift ;;
    --no-cache) NO_CACHE=1; shift ;;
    --no-receipt) NO_RECEIPT=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "gates.sh: unknown argument '$1'" >&2; usage >&2; exit 2 ;;
  esac
done

# ---------------------------------------------------------------------------
# gate definitions -- `--list` prints exactly these, so SKILL.md and the
# reference docs can be checked for drift against the real commands.
# ---------------------------------------------------------------------------
gate_cmd() {
  case "$1" in
    lint)        echo 'uv run ruff check . && uv run ruff format --check .' ;;
    types)       echo 'uv run mypy --strict src/er' ;;
    unit)        echo 'uv run pytest tests/unit -q --maxfail=5' ;;
    dbt)         echo 'uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem' ;;
    board)       echo 'python3 scripts/board.py validate' ;;
    spec)        echo 'python3 scripts/lint_spec.py DesignDoc.md' ;;
    integration) echo 'bash scripts/ci/itest.sh tests/integration -q' ;;
    *) return 1 ;;
  esac
}

FAST_GATES=(spec board lint types unit dbt)
FULL_GATES=(spec board lint types unit dbt integration)

if [[ $LIST_ONLY -eq 1 ]]; then
  for g in "${FULL_GATES[@]}"; do printf '%s\t%s\n' "$g" "$(gate_cmd "$g")"; done
  exit 0
fi

# ---------------------------------------------------------------------------
# scope resolution
#
# CRITICAL: scope must be computed from the WORKING TREE, not from
# `git diff $BASE_REF...HEAD`. Gates run BEFORE the commit, so a three-dot diff
# is empty at exactly the moment we need it and every ticket would silently
# resolve to `fast` -- disabling the only gate that proves anything.
# ---------------------------------------------------------------------------
changed_paths() {
  {
    git diff --name-only 2>/dev/null
    git diff --cached --name-only 2>/dev/null
    git ls-files --others --exclude-standard 2>/dev/null
    if git rev-parse --verify --quiet "$BASE_REF" >/dev/null; then
      git diff --name-only "$BASE_REF"...HEAD 2>/dev/null
    fi
  } | sort -u | sed '/^$/d'
}

ticket_field() {
  local field="$1"
  [[ -z "$TICKET" ]] && return 1
  python3 - "$TICKET" "$field" <<'PY' 2>/dev/null
import json, subprocess, sys
tid, field = sys.argv[1], sys.argv[2]
out = subprocess.run(
    [sys.executable, "scripts/board.py", "show", tid, "--json"],
    capture_output=True, text=True,
)
if out.returncode != 0:
    sys.exit(1)
data = json.loads(out.stdout)
value = data.get(field, "")
print(json.dumps(value) if isinstance(value, list) else value)
PY
}

CHANGED="$(changed_paths)"

if [[ "$SCOPE" == "auto" ]]; then
  declared="$(ticket_field gates || true)"
  if [[ -n "$declared" ]]; then
    SCOPE="$declared"
  else
    SCOPE="fast"
  fi
  # A declared scope is a floor, never a ceiling: touching the pipeline, dbt, the
  # Docker substrate or the integration suite escalates to full regardless.
  if grep -qE '^(src/er/|dbt/|docker/|fixtures/|tests/integration/)' <<<"$CHANGED"; then
    SCOPE="full"
  fi
fi

case "$SCOPE" in
  fast) GATES=("${FAST_GATES[@]}") ;;
  full) GATES=("${FULL_GATES[@]}") ;;
  *) echo "gates.sh: --scope must be fast, full or auto (got '$SCOPE')" >&2; exit 2 ;;
esac

# Bootstrap.
#
# Two gates run tooling that the board itself builds: `integration` needs
# scripts/ci/itest.sh, and `dbt` needs a dbt project. Early in the board neither
# exists. A gate that cannot run is recorded as -2 -- SKIPPED, never PASSED --
# and `board.py complete` refuses -2 for the ticket whose job is to create that
# tooling, so the ticket that owns the substrate cannot skip the gate that proves
# it. Refusing outright instead would deadlock: the ticket that builds the
# integration runner would need the integration runner to pass its own gates.
INTEGRATION_AVAILABLE=1
[[ -f "$REPO_ROOT/scripts/ci/itest.sh" ]] || INTEGRATION_AVAILABLE=0

DBT_AVAILABLE=1
[[ -f "$REPO_ROOT/dbt/dbt_project.yml" ]] || DBT_AVAILABLE=0

# The lint/types/unit gates all run through `uv run`, which with no pyproject.toml
# tries to resolve an environment and can block for minutes. Before the skeleton
# ticket lands there is nothing for them to check anyway.
UV_AVAILABLE=1
[[ -f "$REPO_ROOT/pyproject.toml" ]] || UV_AVAILABLE=0

mkdir -p "$RECEIPTS_DIR" "$LOGS_DIR" "$CACHE_DIR"

# Hash the WORKING TREE, not the index. `git write-tree` writes the index, so a
# receipt built from it would bind `complete` to what was staged while the gates
# actually ran against the working tree -- stage a clean version, leave an
# unstaged hack that makes the suite pass, commit the index, and the trees match.
# A scratch index gives us the tree that was really tested.
# mktemp creates the file; git rejects an existing empty file as an index, so the
# path must be unused. Build a scratch index from the working tree and hash that.
_tmp_index="$(mktemp -t er-gates-index.XXXXXX)"
rm -f "$_tmp_index"
TREE="$(GIT_INDEX_FILE="$_tmp_index" git add -A >/dev/null 2>&1 && \
        GIT_INDEX_FILE="$_tmp_index" git write-tree 2>/dev/null)"
rm -f "$_tmp_index"
if [[ -z "$TREE" ]]; then
  echo "gates.sh: cannot hash the working tree; refusing to write an unbindable receipt" >&2
  exit 3
fi
HEAD_SHA="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
DIRTY_HASH="$(git status --porcelain | shasum -a 256 | cut -d' ' -f1)"

echo "gates: scope=$SCOPE tree=${TREE:0:12} ticket=${TICKET:-none}"

# ---------------------------------------------------------------------------
# Gate 0 -- change hygiene. Runs before anything expensive.
# ---------------------------------------------------------------------------
# Results accumulate as a "name=code;" string rather than an associative array:
# macOS ships bash 3.2, which has no `declare -A`.
GATE_RESULTS=""
FAILED=0

record_gate() { GATE_RESULTS="${GATE_RESULTS}$1=$2;"; }

run_gate0() {
  local rc=0

  # 0a. No forbidden suppressions introduced by this change.
  #
  # Two bugs lived here and both made this gate silently dead:
  #  - `git diff` does not report untracked files, so a brand-new module full of
  #    `# type: ignore` was invisible. New file contents are now included.
  #  - `ticket_field` prints the empty string (exit 0) for an absent key, so the
  #    old `[[ "$allowed" == "[]" ]]` was false for every ticket on the board and
  #    the gate never fired once. Empty and "[]" both mean "nothing allowed".
  local diff_added new_files
  diff_added="$(git diff "$BASE_REF" -- . 2>/dev/null | grep '^+' | grep -v '^+++' || true)"
  # Code files only. Prose that merely *names* the forbidden tokens -- this repo's
  # own gates reference does -- is not a suppression, and scanning docs and fixture
  # data would make the gate cry wolf until someone disabled it.
  new_files="$(git ls-files --others --exclude-standard 2>/dev/null \
               | grep -E '\.(py|sql|sh|toml|cfg|ini)$' \
               | grep -vE '^(\.loop/|\.claude/|docs/)' \
               | while IFS= read -r f; do [[ -f "$f" ]] && cat "$f"; done || true)"
  diff_added="$diff_added
$new_files"
  # NOT ${diff_added//[[:space:]]/}: bash pattern substitution over a multi-megabyte
  # string is quadratic and takes minutes on a repo whose files are mostly untracked.
  if [[ -n "$diff_added" ]]; then
    local allowed hits
    allowed="$(ticket_field allowed_suppressions || true)"
    hits="$(grep -nE "$SUPPRESSION_PATTERNS" <<<"$diff_added" || true)"
    if [[ -n "$hits" && ( -z "$allowed" || "$allowed" == "[]" ) ]]; then
      echo "GATE0 FAIL: this change introduces suppressions:" >&2
      echo "$hits" >&2
      echo "Fix the code, or declare allowed_suppressions on the ticket." >&2
      rc=1
    fi
  fi

  # 0b. Protected paths must not be touched.
  local protected
  protected="$(ticket_field protected_paths || echo '[]')"
  if [[ "$protected" != "[]" && -n "$protected" ]]; then
    while IFS= read -r p; do
      [[ -z "$p" ]] && continue
      if grep -qxF "$p" <<<"$CHANGED"; then
        echo "GATE0 FAIL: '$p' is in the ticket's protected_paths and was modified." >&2
        rc=1
      fi
    done < <(python3 -c 'import json,sys;print("\n".join(json.loads(sys.argv[1])))' "$protected" 2>/dev/null || true)
  fi

  # 0c. The loop may never modify its own machinery.
  if grep -qE '^(\.claude/|scripts/board\.py|scripts/gates\.sh|scripts/run-loop\.sh)' <<<"$CHANGED"; then
    echo "GATE0 FAIL: this change touches the loop's own machinery." >&2
    rc=1
  fi

  # 0c2. Deleting the integration runner would silently downgrade every later
  # ticket's gate scope. Creating or editing it is fine; removing it is not.
  if git diff --diff-filter=D --name-only "$BASE_REF" 2>/dev/null | grep -qxF 'scripts/ci/itest.sh'; then
    echo "GATE0 FAIL: this change deletes scripts/ci/itest.sh, which would disable the integration gate." >&2
    rc=1
  fi

  # 0d. Spec edits require an explicit spec-amendment ticket.
  if grep -qxF 'DesignDoc.md' <<<"$CHANGED"; then
    local kind
    kind="$(ticket_field kind || echo code)"
    if [[ "$kind" != "spec-amendment" ]]; then
      echo "GATE0 FAIL: DesignDoc.md modified by a '$kind' ticket." >&2
      echo "If the spec is wrong, block with class spec_contradiction." >&2
      rc=1
    fi
  fi

  # 0e. The change must stay inside the validated plan.
  #
  # The plan is REQUIRED, not optional. When this was `if [[ -f $PLAN_PATH ]]`,
  # deleting the plan (or never writing one) disabled plan enforcement entirely.
  if [[ -n "$TICKET" && ! -f "$PLAN_PATH" ]]; then
    echo "GATE0 FAIL: no validated change plan at ${PLAN_PATH#$REPO_ROOT/}." >&2
    echo "Write it and run 'board.py plan-check' before implementing." >&2
    rc=1
  fi
  if [[ -f "$PLAN_PATH" && -n "$TICKET" ]]; then
    python3 - "$PLAN_PATH" "$TICKET" <<'PY' || rc=1
import json, subprocess, sys
plan = json.load(open(sys.argv[1]))
tid = sys.argv[2]
if plan.get("ticket") != tid:
    sys.exit(0)  # a stale plan for another ticket is caught by plan-check
planned = {e["path"] for e in plan.get("files", []) if e.get("path")}
planned |= set(plan.get("tests", []))
changed = subprocess.run(
    ["bash", "-c",
     "{ git diff --name-only; git diff --cached --name-only; "
     "git ls-files --others --exclude-standard; } | sort -u"],
    capture_output=True, text=True).stdout.split()
# docs/implementation/ is NOT exempt: the ticket file is the live source of the
# verify command, protected_paths, gates scope and the acceptance criteria that
# every enforcement point reads, so an unplanned edit to it is exactly the change
# that must be caught. board.py writes its own frontmatter transitions and commits
# them separately, so a legitimate run leaves nothing here for this check to see.
stray = [c for c in changed
         if c not in planned
         and not c.startswith(".loop/")]
if stray:
    print("GATE0 FAIL: files changed that the validated plan does not name:",
          file=sys.stderr)
    for s in stray:
        print("  " + s, file=sys.stderr)
    print("Update .loop/change-plan.json and re-run plan-check.", file=sys.stderr)
    sys.exit(1)
PY
  fi

  return $rc
}

echo "--- gate: hygiene"
if run_gate0; then
  record_gate hygiene 0
  echo "    PASS"
else
  record_gate hygiene 1
  FAILED=1
  echo "    FAIL"
fi

# ---------------------------------------------------------------------------
# the ladder
# ---------------------------------------------------------------------------
cache_key() {
  echo "${TREE}-$1"
}

for gate in "${GATES[@]}"; do
  if [[ $FAILED -eq 1 ]]; then
    record_gate "$gate" -1
    continue
  fi
  if [[ "$gate" == "integration" && $INTEGRATION_AVAILABLE -eq 0 ]]; then
    echo "--- gate: integration"
    echo "    SKIPPED: scripts/ci/itest.sh does not exist yet (a later ticket creates it)."
    echo "    Recorded as -2 in the receipt, not as a pass."
    record_gate integration -2
    continue
  fi
  if [[ "$gate" == "dbt" && $DBT_AVAILABLE -eq 0 ]]; then
    echo "--- gate: dbt"
    echo "    SKIPPED: dbt/dbt_project.yml does not exist yet (a later ticket creates it)."
    echo "    Recorded as -2 in the receipt, not as a pass."
    record_gate dbt -2
    continue
  fi
  case "$gate" in
    lint|types|unit)
      if [[ $UV_AVAILABLE -eq 0 ]]; then
        echo "--- gate: $gate"
        echo "    SKIPPED: pyproject.toml does not exist yet (the skeleton ticket creates it)."
        echo "    Recorded as -2 in the receipt, not as a pass."
        record_gate "$gate" -2
        continue
      fi
      ;;
  esac
  cmd="$(gate_cmd "$gate")"
  cache_file="$CACHE_DIR/$(cache_key "$gate")"

  # An unchanged tree cannot produce a different gate result. Without this, a
  # three-cycle fix loop pays for the Compose integration run three times.
  if [[ $NO_CACHE -eq 0 && -f "$cache_file" ]]; then
    echo "--- gate: $gate (cached, tree unchanged)"
    record_gate "$gate" 0
    continue
  fi

  echo "--- gate: $gate"
  echo "    $cmd"
  log="$LOGS_DIR/${TICKET:-nogate}-${gate}.log"
  bash -c "$cmd" 2>&1 | tee "$log"
  rc=${PIPESTATUS[0]}
  record_gate "$gate" "$rc"
  if [[ $rc -eq 0 ]]; then
    echo "    PASS"
    : > "$cache_file"
  else
    echo "    FAIL (exit $rc) -- log: ${log#$REPO_ROOT/}"
    FAILED=1
  fi
done

# ---------------------------------------------------------------------------
# the ticket's own verify command, run verbatim
# ---------------------------------------------------------------------------
VERIFY_EXIT=-1
VERIFY_CMD=""
if [[ -n "$TICKET" ]]; then
  VERIFY_CMD="$(python3 "$BOARD" verify-cmd "$TICKET" 2>/dev/null || true)"
  if [[ -n "$VERIFY_CMD" && $FAILED -eq 0 ]]; then
    echo "--- verify: $TICKET"
    echo "    $VERIFY_CMD"
    log="$LOGS_DIR/${TICKET}-verify.log"
    bash -c "$VERIFY_CMD" 2>&1 | tee "$log"
    VERIFY_EXIT=${PIPESTATUS[0]}
    if [[ $VERIFY_EXIT -eq 0 ]]; then
      echo "    PASS"
    else
      echo "    FAIL (exit $VERIFY_EXIT) -- log: ${log#$REPO_ROOT/}"
      FAILED=1
    fi
  fi
fi

# ---------------------------------------------------------------------------
# receipt
# ---------------------------------------------------------------------------
if [[ -n "$TICKET" && $NO_RECEIPT -eq 0 ]]; then
  ATTEMPT="$(ticket_field attempts || echo 0)"
  RECEIPT="$RECEIPTS_DIR/${TICKET}-${ATTEMPT}.json"
  python3 - "$RECEIPT" "$TREE" "$HEAD_SHA" "$DIRTY_HASH" "$SCOPE" "$VERIFY_CMD" "$VERIFY_EXIT" \
      "$GATE_RESULTS" <<'PY'
import json, sys, time
path, tree, head, dirty, scope, vcmd, vexit, codes = sys.argv[1:9]
exit_codes = {}
for pair in codes.strip(";").split(";"):
    if not pair:
        continue
    name, _, value = pair.partition("=")
    exit_codes[name] = int(value)
json.dump({
    "tree": tree,
    "head": head,
    "dirty": dirty,
    "scope": scope,
    "gates_run": sorted(exit_codes),
    "exit_codes": exit_codes,
    "verify_cmd": vcmd,
    "verify_exit": int(vexit),
    "written_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
}, open(path, "w"), indent=2)
print(f"receipt: {path}")
PY
fi

if [[ $FAILED -eq 1 ]]; then
  echo "gates: FAILED"
  exit 1
fi
echo "gates: all passed (scope=$SCOPE)"
exit 0
