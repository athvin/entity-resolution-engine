#!/usr/bin/env bash
# Regression tests for the parts of `skills-check.sh` whose failure mode is a
# RACE, so they cannot be trusted to a single green run.
#
# WHY THIS EXISTS. `names_skill_in_backticks` was
#
#     skill_body "$file" | grep -qF "\`$skill\`"
#
# under `set -euo pipefail`. `grep -q` exits the instant it matches, closing the
# pipe while `awk` is still writing the rest of the body; `awk` takes SIGPIPE and
# the pipeline exits 141. A match that WAS found reports as missing.
#
# That is the worst shape a gate can have: it passed on every developer machine
# and on most CI runs, then failed once on #606 claiming
# `frontend-design-system` never names `frontend-design` — against a tree where
# it does, at body line 26 — and passed again on rerun. Three independent
# reproductions of the exact `refs/pull/606/merge` tree all passed locally,
# which is precisely what a race looks like from the outside.
#
# The repository already knew this pattern. `tf-helpers-test.sh` carries a
# comment avoiding it, and the ci-and-supply-chain skill states the rule —
# `grep -c`, not `grep -q`, on anything fed by a pipe, because a false SKIP is
# worse than a failure. `skills-check.sh` just had an instance nobody had
# connected to it.
#
# Each test below is DETERMINISTIC: it drives enough body through the pipe that
# the old implementation must block on a full pipe buffer and therefore must
# take SIGPIPE. The suite also asserts that the old implementation FAILS these
# cases, because a regression test that cannot distinguish the fix from the bug
# proves nothing — the lesson from every gate in this repo that was green for
# the wrong reason.

set -euo pipefail

SCRIPT="${1:-scripts/skills-check.sh}"
[ -f "$SCRIPT" ] || {
    printf 'skills-check-test: cannot find %s (run from the repository root)\n' "$SCRIPT" >&2
    exit 2
}

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

# Lift the two functions under test out of the real script, so this exercises
# the shipped code rather than a copy of it. Sourcing the script itself is not
# an option: it runs its `case` dispatcher at the bottom.
extract() { sed -n "/^$1()/,/^}/p" "$SCRIPT"; }

for fn in skill_body names_skill_in_backticks; do
    if [ -z "$(extract "$fn")" ]; then
        printf 'skills-check-test: %s() not found in %s — was it renamed?\n' "$fn" "$SCRIPT" >&2
        exit 2
    fi
done

# A skill file with the match EARLY and a body far larger than one pipe buffer.
# `filler` is tuned to exceed 64 KiB after the match, which makes the SIGPIPE
# certain rather than likely.
make_skill() { # $1 path, $2 name-to-mention or "" for none, $3 filler lines
    {
        printf -- '---\nname: house\ndescription: d\n---\n\n'
        [ -n "$2" ] && printf 'This skill **supersedes the `%s` skill** on one claim.\n' "$2"
        seq 1 "$3" | sed 's/^/filler line /;s/$/ — padding so the writer is still writing/'
    } >"$1"
}

big_body_bytes=$(
    make_skill "$tmp/big.md" "frontend-design" 4000
    wc -c <"$tmp/big.md" | tr -d ' '
)
make_skill "$tmp/big.md" "frontend-design" 4000
make_skill "$tmp/absent.md" "" 4000
make_skill "$tmp/small.md" "dataviz" 3

# --- the fixed implementation ------------------------------------------------

fixed() {
    bash -c '
        set -euo pipefail
        '"$(extract skill_body)"'
        '"$(extract names_skill_in_backticks)"'
        names_skill_in_backticks "$1" "$2"
    ' _ "$1" "$2"
}

if fixed "$tmp/big.md" "frontend-design"; then
    ok "finds a name early in a ${big_body_bytes}-byte body (the SIGPIPE case)"
else
    no "finds a name early in a ${big_body_bytes}-byte body (the SIGPIPE case)" \
        "this is the #606 failure: the name IS present and the check denies it"
fi

if fixed "$tmp/small.md" "dataviz"; then
    ok "finds a name in a small body"
else
    no "finds a name in a small body"
fi

# Negative control. Without this the suite would pass just as happily against a
# function that returns true unconditionally.
if fixed "$tmp/absent.md" "frontend-design"; then
    no "reports a name that is genuinely absent" "false positive — the check no longer discriminates"
else
    ok "still reports a genuinely absent name as missing"
fi

# --- mutation: the OLD implementation must fail the first case ---------------
#
# If this passes, the test above is not measuring what it claims to.

old_impl_result=$(
    bash -c '
        set -euo pipefail
        '"$(extract skill_body)"'
        old() { skill_body "$1" | grep -qF "\`$2\`"; }
        if old "$1" "$2"; then echo pass; else echo fail; fi
    ' _ "$tmp/big.md" "frontend-design" 2>/dev/null || echo fail
)
if [ "$old_impl_result" = fail ]; then
    ok "the pre-fix 'skill_body | grep -qF' implementation DOES fail this case"
else
    no "the pre-fix implementation also passes — this suite does not discriminate" \
        "the SIGPIPE was not provoked; raise the filler line count"
fi

# --- no new instances of the pattern in the gate -----------------------------
#
# The fix is one line away from being reintroduced by anyone who finds the
# captured-then-matched form verbose.

piped_q=$(grep -n '| *grep -q' "$SCRIPT" | grep -v "printf '%s' \"\$desc\"" | grep -cv 'head -3' || true)
if [ "${piped_q:-0}" -eq 0 ]; then
    ok "no unbounded writer is piped into 'grep -q' in $SCRIPT"
else
    no "an unbounded writer is piped into 'grep -q' in $SCRIPT" \
        "$(grep -n '| *grep -q' "$SCRIPT" | grep -v "printf '%s' \"\$desc\"" | grep -v 'head -3')"
fi

printf '\nskills-check-test: %s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
