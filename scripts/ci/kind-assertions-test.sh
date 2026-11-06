#!/usr/bin/env bash
# Fixture tests for the kind probe's assertion blocks, so a change that
# NEUTERS one is caught on the PR that makes it rather than never.
#
# WHY THIS EXISTS. `scripts/check_kind.sh` needs a real cluster, a real image
# build and roughly two hours, and it runs nightly — so its python assertion
# blocks are the least-exercised correctness-critical code in the repository.
# That would be tolerable if the failure mode were loud. It is not: an
# assertion that can no longer fail makes the nightly probe GREENER, not
# redder, so the nightly cannot detect its own neutering. Only a mutation test
# can, and only a mutation test that runs per-PR catches it before it lands.
#
# This is not hypothetical. Phase 6 was written asserting
# `termination_reason != "control_plane_unreachable"` on a task caught in
# `running`. Reading exec_task.rs afterwards showed that reason is written at
# exactly one site — the step-1 work-order loop — which a running task has
# already passed; the reachable mid-run failure is `fenced`, from the heartbeat
# thread. The guard's precondition could not hold, so it would have passed no
# matter what the cutover did, through a full two-hour green run. The
# assertion is now `termination_reason is None` and every branch of it is
# mutated below.
#
# The blocks are EXTRACTED from check_kind.sh rather than transcribed. A copy
# would be a second source of truth that drifts, and the drift would show up as
# this test passing against code the probe no longer runs.
#
# No cluster, no network, no docker. Runs in about a second.
#
# Usage: scripts/ci/kind-assertions-test.sh
# Exit codes: 0 all assertions held; 1 at least one did not; 2 nothing examined.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT" || exit 1

PROBE="scripts/check_kind.sh"
[ -f "$PROBE" ] || {
    printf 'kind-assertions-test: missing %s\n' "$PROBE" >&2
    exit 2
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fails=0
ok() { printf '  PASS  %s\n' "$1"; }
no() {
    printf '  FAIL  %s\n' "$1" >&2
    fails=$((fails + 1))
}

# ---- every embedded block must at least compile ---------------------------
# Cheap coverage for the phases whose assertions this test does not mutate:
# a syntax error in any of them would otherwise surface two hours into a
# nightly run, after the image build and five phases of real work.
blocks="$(python3 - "$PROBE" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
print(len(re.findall(r"python3 - /tmp/er-kind-tasks\d+\.json <<'EOF'\n.*?\nEOF\n", src, re.S)))
PY
)"
if [ "${blocks:-0}" -lt 2 ]; then
    no "found only ${blocks:-0} embedded assertion blocks — the heredoc shape changed, so this test examines nothing"
    printf 'kind-assertions-test: nothing to examine\n' >&2
    exit 2
fi
if python3 - "$PROBE" <<'PY'
import re, sys, py_compile, tempfile, os
src = open(sys.argv[1]).read()
bad = []
for i, m in enumerate(re.finditer(r"python3 - (/tmp/er-kind-tasks\d+\.json) <<'EOF'\n(.*?)\nEOF\n", src, re.S)):
    f = tempfile.NamedTemporaryFile("w", suffix=".py", delete=False)
    f.write(m.group(2)); f.close()
    try:
        py_compile.compile(f.name, doraise=True)
    except Exception as e:
        bad.append(f"{m.group(1)}: {e}")
    finally:
        os.unlink(f.name)
sys.exit(1 if bad else 0)
PY
then
    ok "all ${blocks} embedded assertion blocks compile"
else
    no "an embedded assertion block does not compile"
fi

# ---- phase 6: mutate every branch ----------------------------------------
# Each case feeds a crafted task ledger to the REAL extracted block and
# asserts the exit code. A mutant that still exits 0 is an assertion that
# cannot discriminate, which is the whole thing being tested for.
if python3 - "$PROBE" "$tmp" <<'PY'
import json, os, re, subprocess, sys

probe, tmp = sys.argv[1], sys.argv[2]
src = open(probe).read()
m = re.search(r"python3 - /tmp/er-kind-tasks6\.json <<'EOF'\n(.*?)\nEOF\n", src, re.S)
if not m:
    print("  FAIL  phase 6 assertion block not found in check_kind.sh", file=sys.stderr)
    sys.exit(2)
blk = os.path.join(tmp, "phase6.py")
open(blk, "w").write(m.group(1))


def row(**kw):
    # DEFAULTS MODEL THE ANNOTATED LEDGER, not the moment before it. The
    # executor annotates an already-succeeded row with the pod's terminated
    # reason, so the steady-state success row reads
    # termination_reason="Completed", exit_code=0 — and defaulting this to
    # None is exactly how the `is None` guard shipped and failed a green
    # cutover. The unannotated shape is still covered, as its own case below.
    r = {
        "task_id": "maintain",
        "live": True,
        "state": "succeeded",
        "termination_reason": "Completed",
        "exit_code": 0,
        "launches": 1,
        "error_detail": None,
    }
    r.update(kw)
    return r


# (label, ledger, should the block exit 0, required substring of its output)
#
# That fourth element is not decoration. The `assert "termination_reason" in t`
# guard in the probe is BEHAVIOURALLY redundant — the subscript beside it
# raises KeyError on a missing field either way — so deleting it leaves every
# exit code unchanged and no mutant can catch it. What it actually buys is the
# diagnosis: a named AssertionError about the ledger shape instead of a bare
# KeyError traceback surfacing two hours into a nightly run. Asserting the
# message is the only way to keep it.
cases = [
    # The annotated success row — what the live ledger actually holds, and the
    # case that reproduces the 2026-11-06 nightly failure: the previous
    # `is None` guard refused this and failed the phase for succeeding.
    ("a clean cutover passes (annotated Completed)", [row()], True, None),
    # The same attempt read before the executor annotates it. Both are
    # undisrupted, and which one a run sees is a race, so both must pass or
    # the phase is flaky rather than correct.
    (
        "a clean cutover passes (not yet annotated)",
        [row(termination_reason=None, exit_code=None)],
        True,
        None,
    ),
    # FAIL-SAFE: a reason nobody has enumerated must refuse. A denylist of
    # today's failures would accept anything added to exec_task.rs later,
    # which is the defect where an omission reads as permission.
    (
        "REFUSES an unknown future reason",
        [row(termination_reason="some_new_reason_added_later")],
        False,
        None,
    ),
    # "Completed" is the POD's reason, so a nonzero exit contradicts it. A
    # succeeded row carrying one is incoherent however it got there.
    (
        "REFUSES succeeded with a nonzero exit_code",
        [row(exit_code=1)],
        False,
        "exit_code",
    ),
    # The reachable mid-run failure: three missed heartbeats fence the attempt,
    # step 6 refuses to post a verdict, the finished work is discarded.
    ("REFUSES fenced", [row(termination_reason="fenced")], False, None),
    # Only reachable by a sibling that launches after the flip and cannot
    # fetch its work order through the moved Service.
    ("REFUSES control_plane_unreachable", [row(termination_reason="control_plane_unreachable")], False, None),
    ("REFUSES sigterm", [row(termination_reason="sigterm")], False, None),
    ("REFUSES stale_start", [row(termination_reason="stale_start")], False, None),
    # Two processes writing one lake — the invariant phase 2 also protects.
    ("REFUSES a relaunched attempt", [row(launches=2)], False, None),
    ("REFUSES a task that did not succeed", [row(state="failed")], False, None),
    # Rename detection, and it must DIAGNOSE rather than merely die: see the
    # note above on why the exit code alone cannot keep this guard.
    (
        "REFUSES a ledger missing termination_reason, by name",
        [{k: v for k, v in row().items() if k != "termination_reason"}],
        False,
        "termination_reason field",
    ),
    # The cutover window caught nothing, so the phase proved nothing.
    ("REFUSES an empty live set", [row(live=False)], False, None),
    # One bad row among good ones must not be averaged away.
    ("REFUSES one fenced row among many", [row(), row(task_id="b", termination_reason="fenced")], False, None),
]

fails = 0
for label, tasks, want_pass, want_text in cases:
    f = os.path.join(tmp, "t6.json")
    with open(f, "w") as fh:
        json.dump({"tasks": tasks}, fh)
    p = subprocess.run([sys.executable, blk, f], capture_output=True, text=True)
    passed = p.returncode == 0
    out = (p.stdout or "") + (p.stderr or "")
    tail = [l for l in out.strip().splitlines() if l.strip()]
    why = tail[-1][:100] if tail else "(no output)"
    if passed != want_pass:
        fails += 1
        got = "exited 0" if passed else "refused"
        print(f"  FAIL  phase 6 {label}: {got}, wanted {'exit 0' if want_pass else 'a refusal'} — {why}", file=sys.stderr)
    elif want_text and want_text not in out:
        fails += 1
        print(
            f"  FAIL  phase 6 {label}: refused, but without naming {want_text!r} "
            f"— a bare traceback is the wrong diagnosis for a nightly run; got: {why}",
            file=sys.stderr,
        )
    else:
        print(f"  PASS  phase 6 {label}")

sys.exit(1 if fails else 0)
PY
then
    ok "phase 6 assertions discriminate on every mutated branch"
else
    no "a phase 6 mutant was not refused — that assertion cannot fail"
fi

# ---- every /v1 path the probe calls must be a declared route --------------
# The probe spent two days calling `POST /v1/orgs/{org}/jobs`, which stopped
# existing when the datasets work made every job route dataset-scoped
# (2026-11-04). Nothing noticed: kind.yml is nightly and not required, two of
# its runs were outage no-starts, and the one that did report surfaced it as a
# bare JSONDecodeError. A 404 against a route that is right there in the router
# is the cheapest possible thing to check, and it needs no cluster.
if python3 - "$PROBE" <<'PY'
import re, sys

probe = open(sys.argv[1]).read()
router = open("rust/crates/er-server/src/lib.rs").read()

routes = set(re.findall(r'"(/v1/[^"]*)"', router))
if len(routes) < 20:
    print(f"  FAIL  found only {len(routes)} /v1 routes in the router — the "
          f"declaration shape changed, so this check examines nothing",
          file=sys.stderr)
    sys.exit(2)

# Each shell interpolation maps to the placeholder the router declares. Keep
# this table explicit: a silent fallback would turn an unmapped variable into
# a false pass, which is the failure mode this whole file exists to prevent.
SUBST = {
    "${ORG}": "{org}", "${PORG}": "{org}", "${DS}": "{ds}",
    "${JOB3}": "{job_id}", "${JOB6}": "{job_id}", "${id}": "{job_id}",
    "$1": "{job_id}", "${ATTEMPT}": "{attempt_id}",
}

paths = set()
for m in re.finditer(r'api (?:POST|GET|DELETE) "(/v1/[^"]*)"', probe):
    paths.add(m.group(1))
for m in re.finditer(r'\$\{BASE\}(/v1/[^"\s)]*)', probe):
    paths.add(m.group(1))
if not paths:
    print("  FAIL  no /v1 call sites found in the probe — it cannot be calling "
          "nothing, so the extraction is broken", file=sys.stderr)
    sys.exit(2)

bad, unmapped = [], []
for p in sorted(paths):
    n = p
    for k, v in SUBST.items():
        n = n.replace(k, v)
    if "$" in n:
        unmapped.append((p, n))
    elif n not in routes:
        bad.append((p, n))

for p, n in unmapped:
    print(f"  FAIL  {p} contains an interpolation this check cannot map ({n}) — "
          f"add it to SUBST rather than leaving it unchecked", file=sys.stderr)
for p, n in bad:
    print(f"  FAIL  the probe calls {p}, which normalises to {n} — no such "
          f"route in er-server. It will 404.", file=sys.stderr)
if not bad and not unmapped:
    print(f"  (checked {len(paths)} probe paths against {len(routes)} routes)")
sys.exit(1 if (bad or unmapped) else 0)
PY
then
    ok "every /v1 path the probe calls is a declared route"
else
    no "the probe calls a route er-server does not declare"
fi

# ---- the cutover guards are present at all -------------------------------
# Textual, deliberately: these four live in bash rather than python, and the
# cost of asserting them properly is a cluster. A grep cannot prove they work,
# but it can prove nobody deleted them, which is the realistic regression.
#
# The CPU preflight is in this list for a reason worth stating: deleting it
# does not make phase 6 fail, it makes phase 6 fail LATER and about something
# else — a `helm --wait` timeout, or a job that "never reached succeeded" —
# which is precisely the misattribution it was added to prevent. A guard whose
# removal degrades the diagnosis rather than the verdict is invisible to every
# other check here.
for needle in \
    'carries no er.dupezero.com/color selector' \
    'cutover did not take' \
    'restore failed' \
    'needs ${need}m of CPU and only ${free}m is free'; do
    if grep -qF "$needle" "$PROBE"; then
        ok "the guard '${needle}' is still present"
    else
        no "the guard '${needle}' is gone from ${PROBE}"
    fi
done

printf '\n'
if [ "$fails" -eq 0 ]; then
    printf 'kind-assertions-test: ok\n'
    exit 0
fi
printf 'kind-assertions-test: %d assertion(s) did not hold\n' "$fails" >&2
exit 1
