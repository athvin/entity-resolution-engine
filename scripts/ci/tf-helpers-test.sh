#!/usr/bin/env bash
# Regression tests for the three terraform-CI helpers, so their behaviour is
# asserted on every infra PR rather than on the day someone needs it.
#
# WHY THIS EXISTS. `tf-destroy-guard.sh` is the only thing standing between a
# merge and an unattended `terraform apply` that destroys production, and §14.2's
# prod-scoped apply role is not built yet. Every real apply so far has planned
# `0 to destroy`, so the guard has never actually refused anything in CI — its
# refusal path was verified once, by hand, by the person who wrote it. That is
# exactly the state this repository has been burned by before: a gate nobody has
# watched fail (semgrep reporting zero findings on a parse error, a Playwright
# exit code swallowed by `| tail`). A gate whose failure path is unexercised is
# indistinguishable from a gate that cannot fail.
#
# So the refusal, the token override and the fail-closed branch are asserted
# here, with fixtures, on every run. No network, no AWS, no terraform binary.
#
# Usage: scripts/ci/tf-helpers-test.sh
# Exit codes: 0 all assertions held; 1 at least one did not.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT" || exit 1

GUARD="scripts/ci/tf-destroy-guard.sh"
META="scripts/ci/tf-plan-meta.sh"
COMMENT="scripts/ci/tf-plan-comment.sh"

for f in "$GUARD" "$META" "$COMMENT"; do
    [ -f "$f" ] || {
        printf 'tf-helpers-test: missing %s\n' "$f" >&2
        exit 1
    }
done

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0

ok() {
    pass=$((pass + 1))
    printf '  PASS  %s\n' "$1"
}
no() {
    fail=$((fail + 1))
    printf '  FAIL  %s\n' "$1" >&2
    [ -n "${2:-}" ] && printf '        %s\n' "$2" >&2
    return 0
}

# $1 label, $2 expected exit, then the command
expect_exit() {
    local label=$1 want=$2
    shift 2
    local out got
    out="$("$@" 2>&1)"
    got=$?
    if [ "$got" = "$want" ]; then
        ok "$label"
    else
        no "$label" "exit $got, wanted $want — output: $(printf '%s' "$out" | head -2 | tr '\n' ' ')"
    fi
}

# $1 label, $2 expected stdout (exact), then the command
expect_out() {
    local label=$1 want=$2
    shift 2
    local got
    got="$("$@" 2>/dev/null)"
    if [ "$got" = "$want" ]; then
        ok "$label"
    else
        no "$label" "got '$got', wanted '$want'"
    fi
}

# --- fixtures ---------------------------------------------------------------

cat >"$tmp/no-changes.txt" <<'EOF'
No changes. Your infrastructure matches the configuration.
EOF

cat >"$tmp/adds.txt" <<'EOF'
  # aws_iam_role.foo will be created
  + resource "aws_iam_role" "foo" {
      + name = "foo"
    }

Plan: 2 to add, 1 to change, 0 to destroy.
EOF

cat >"$tmp/destroy.txt" <<'EOF'
  # aws_rds_cluster.prod will be destroyed
  - resource "aws_rds_cluster" "prod" {
      - cluster_identifier = "er-prod" -> null
    }

Plan: 0 to add, 0 to change, 1 to destroy.
EOF

# Terraform prints the import count FIRST, so a pattern anchored on the whole
# summary line matches nothing here and would fall through to "unparseable".
cat >"$tmp/import-destroy.txt" <<'EOF'
Plan: 1 to import, 0 to add, 0 to change, 3 to destroy.
EOF

# An output-only diff prints no `Plan:` line at all. A real state, not garbage.
cat >"$tmp/outputs.txt" <<'EOF'
Changes to Outputs:
  ~ ci_tf_plan_role_arn = "arn:aws:iam::1:role/a" -> "arn:aws:iam::1:role/b"
EOF

cat >"$tmp/garbage.txt" <<'EOF'
Error: Failed to load state
truncated, with no summary of any kind
EOF

printf 'fix(infra): an ordinary change\n\nnothing scary\n' >"$tmp/msg-plain.txt"
printf 'fix(infra)!: replace the bucket ALLOW-DESTROY\n\nbody\n' >"$tmp/msg-token.txt"

# --- tf-destroy-guard.sh ----------------------------------------------------

printf '\ntf-destroy-guard.sh\n'
expect_exit "allows an empty plan"              0 bash "$GUARD" "$tmp/no-changes.txt" "$tmp/msg-plain.txt"
expect_exit "allows adds and changes"           0 bash "$GUARD" "$tmp/adds.txt" "$tmp/msg-plain.txt"
expect_exit "allows an outputs-only diff"       0 bash "$GUARD" "$tmp/outputs.txt" "$tmp/msg-plain.txt"
# THE LOAD-BEARING ONE. If this ever starts passing, an unattended apply will
# tear down whatever a merge happens to plan.
expect_exit "REFUSES a destroy without the token" 1 bash "$GUARD" "$tmp/destroy.txt" "$tmp/msg-plain.txt"
expect_exit "allows a destroy WITH the token"     0 bash "$GUARD" "$tmp/destroy.txt" "$tmp/msg-token.txt"
expect_exit "refuses an import-form destroy"      1 bash "$GUARD" "$tmp/import-destroy.txt" "$tmp/msg-plain.txt"
expect_exit "FAILS CLOSED on unparseable input"   2 bash "$GUARD" "$tmp/garbage.txt" "$tmp/msg-plain.txt"
expect_exit "rejects a missing plan file"         2 bash "$GUARD" "$tmp/does-not-exist.txt" "$tmp/msg-plain.txt"
expect_exit "rejects no arguments"                2 bash "$GUARD"

# The refusal has to say what to do about it, or the next person force-merges.
#
# Captured into a variable and THEN grepped, not piped. `set -o pipefail` is on
# in this file, so `guard | grep -q` would return the guard's own exit 1 — which
# is the behaviour under test — and the assertion would read false however
# correct the message was. `grep -q` would also SIGPIPE the guard on the way.
refusal="$(bash "$GUARD" "$tmp/destroy.txt" "$tmp/msg-plain.txt" 2>&1 || true)"
case "$refusal" in
*ALLOW-DESTROY*) ok "names ALLOW-DESTROY in the refusal" ;;
*) no "names ALLOW-DESTROY in the refusal" "message: $refusal" ;;
esac

# --- tf-plan-meta.sh --------------------------------------------------------

printf '\ntf-plan-meta.sh\n'
expect_out "exit 0 means changes=false" \
    "$(printf 'status=ok\nchanges=false\nadd=0\nchange=0\ndestroy=0')" \
    bash "$META" "$tmp/no-changes.txt" 0
expect_out "exit 2 parses the counts" \
    "$(printf 'status=ok\nchanges=true\nadd=2\nchange=1\ndestroy=0')" \
    bash "$META" "$tmp/adds.txt" 2
expect_out "reads the import form per-clause" \
    "$(printf 'status=ok\nchanges=true\nadd=0\nchange=0\ndestroy=3')" \
    bash "$META" "$tmp/import-destroy.txt" 2
expect_out "exit 1 means status=error" \
    "$(printf 'status=error\nchanges=false\nadd=\nchange=\ndestroy=')" \
    bash "$META" "$tmp/garbage.txt" 1
expect_exit "rejects a missing plan file" 2 bash "$META" "$tmp/does-not-exist.txt" 0
expect_exit "rejects no arguments" 2 bash "$META"

# --- tf-plan-comment.sh -----------------------------------------------------

printf '\ntf-plan-comment.sh\n'

# Build the artifact layout `actions/download-artifact` lays down for
# `pattern: plan-*  path: <dir>`: one directory per artifact.
leg() { # $1 dir, $2 key, $3 plan fixture, $4.. meta lines
    local d="$1/plan-$2"
    mkdir -p "$d"
    cp "$3" "$d/plan.txt"
    shift 3
    printf '%s\n' "$@" >"$d/meta.txt"
}

clean="$tmp/arts-clean"
for k in state-backend core dev prod; do
    leg "$clean" "$k" "$tmp/no-changes.txt" status=ok changes=false add=0 change=0 destroy=0
done
body="$tmp/body-clean.md"
if bash "$COMMENT" "$clean" >"$body" 2>/dev/null; then ok "renders an all-clean report"; else no "renders an all-clean report"; fi
grep -q '<!-- terraform-plan -->' "$body" && ok "emits the upsert marker" || no "emits the upsert marker"
grep -q 'state-backend` _(plan only)_' "$body" && ok "marks the plan-only stack" || no "marks the plan-only stack"
# A clean stack gets no <details> block; otherwise every no-op PR carries four
# empty ones.
[ "$(grep -c '<details' "$body")" = "0" ] && ok "no detail block for a clean stack" || no "no detail block for a clean stack"

# A destroy on an APPLIED stack warns; on a plan-only stack it must not, because
# the warning's whole claim is "merging applies this".
warn="$tmp/arts-warn"
leg "$warn" prod "$tmp/destroy.txt" status=ok changes=true add=0 change=0 destroy=1
bash "$COMMENT" "$warn" >"$tmp/body-warn.md" 2>/dev/null
grep -q '\[!WARNING\]' "$tmp/body-warn.md" && ok "warns on a destroy in an applied env" || no "warns on a destroy in an applied env"
grep -q '| \*\*1\*\* |' "$tmp/body-warn.md" && ok "bolds a non-zero destroy count" || no "bolds a non-zero destroy count"

planonly="$tmp/arts-planonly"
leg "$planonly" state-backend "$tmp/destroy.txt" status=ok changes=true add=0 change=0 destroy=1
bash "$COMMENT" "$planonly" >"$tmp/body-po.md" 2>/dev/null
grep -q '\[!WARNING\]' "$tmp/body-po.md" && no "no apply warning for a plan-only destroy" || ok "no apply warning for a plan-only destroy"

# A failed plan must still be reported — that is when the comment matters most.
failed="$tmp/arts-failed"
leg "$failed" dev "$tmp/garbage.txt" status=error changes=false add= change= destroy=
bash "$COMMENT" "$failed" >"$tmp/body-failed.md" 2>/dev/null
grep -q 'plan failed' "$tmp/body-failed.md" && ok "reports a failed plan" || no "reports a failed plan"

# An env the display order does not know about must be appended, not dropped.
unknown="$tmp/arts-unknown"
leg "$unknown" staging "$tmp/adds.txt" status=ok changes=true add=2 change=1 destroy=0
bash "$COMMENT" "$unknown" >"$tmp/body-unknown.md" 2>/dev/null
grep -q 'staging' "$tmp/body-unknown.md" && ok "appends an unknown env rather than dropping it" || no "appends an unknown env rather than dropping it"

# THE CAP. GitHub rejects a comment body over 65536 characters outright, so a
# large plan would turn the review surface off exactly when it matters.
big="$tmp/arts-big"
{
    i=0
    while [ "$i" -lt 2600 ]; do
        printf '  # aws_iam_role.r%s will be created\n  + resource "aws_iam_role" "r%s" { name = "r%s" }\n' "$i" "$i" "$i"
        i=$((i + 1))
    done
    printf 'Plan: 2600 to add, 0 to change, 0 to destroy.\n'
} >"$tmp/huge.txt"
for k in state-backend core dev prod; do
    leg "$big" "$k" "$tmp/huge.txt" status=ok changes=true add=2600 change=0 destroy=0
done
bash "$COMMENT" "$big" >"$tmp/body-big.md" 2>/dev/null
size="$(wc -c <"$tmp/body-big.md" | tr -d ' ')"
if [ "$size" -lt 65536 ]; then
    ok "four oversized plans stay under the 65536 cap ($size bytes)"
else
    no "four oversized plans stay under the 65536 cap" "$size bytes"
fi
grep -q 'Showing the last' "$tmp/body-big.md" && ok "says so when it truncates" || no "says so when it truncates"

# Plan text can contain a triple backtick inside a string value; a backtick
# fence would close early and spill raw plan text into the comment as markdown.
fence="$tmp/arts-fence"
printf '  ~ user_data = <<-EOT\n      echo "```"\n    EOT\nPlan: 0 to add, 1 to change, 0 to destroy.\n' >"$tmp/fence.txt"
leg "$fence" core "$tmp/fence.txt" status=ok changes=true add=0 change=1 destroy=0
bash "$COMMENT" "$fence" >"$tmp/body-fence.md" 2>/dev/null
[ "$(grep -c '^~~~~' "$tmp/body-fence.md")" = "2" ] && ok "a backtick in plan text does not break the fence" || no "a backtick in plan text does not break the fence"

# No artifacts at all is what a detect-skipped run looks like: exit 2 so the
# caller posts nothing rather than an empty table.
mkdir -p "$tmp/arts-empty"
expect_exit "exit 2 when there are no artifacts" 2 bash "$COMMENT" "$tmp/arts-empty"
expect_exit "exit 2 for a missing directory" 2 bash "$COMMENT" "$tmp/no-such-dir"
expect_exit "exit 2 for no arguments" 2 bash "$COMMENT"

# --- verdict ----------------------------------------------------------------

printf '\ntf-helpers-test: %s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
