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

# --- the description-cap warning must stay a WARNING -------------------------
#
# Not race-shaped, but the same silent-breakage shape as everything above: most
# skills already sit inside the warning band, so promoting `warn` to `err` here
# turns a clean tree red on an unrelated change. Asserted structurally — there
# is no way to point the gate at a fixture skills tree, since it cds to the
# repository root and scans `.claude/skills` directly.

if [ -z "$(extract warn)" ]; then
    no "warn() not found in $SCRIPT — was it renamed?"
elif extract warn | grep -q 'fail=1'; then
    no "warn() sets fail=1 — a warning that fails the gate is an error" \
        "$(extract warn)"
else
    ok "warn() reports without failing the gate"
fi

cap_line=$(grep -n 'from the 1024 cap' "$SCRIPT" | head -1 || true)
if [ -z "$cap_line" ]; then
    no "the description-headroom check is gone from $SCRIPT"
elif printf '%s' "$cap_line" | grep -q 'err '; then
    no "the headroom check calls err, not warn" "$cap_line"
else
    ok "the description-headroom check calls warn, not err"
fi

if grep -q 'err "\$slug: description is \${#desc} chars (max 1024)"' "$SCRIPT"; then
    ok "the hard 1024-char cap is still an error"
else
    no "the hard 1024-char cap no longer calls err" \
        "the warning is not a substitute for the cap — both must exist"
fi

# --- the YAML-comment truncation rule ---------------------------------------
#
# `project-board`'s description contained `project #2` and so ENDED there: YAML
# opens a comment at ` #`, with no parse error, and both Claude Code and Codex
# were shown 117 of its 989 characters. The gate could not see it because
# `frontmatter_value` reads the raw line.
#
# The predicate is lifted and driven from both sides, because the way a rule
# like this dies is by quietly matching nothing — which reads exactly like a
# clean tree. The negative cases are the ones that matter: a `#` NOT preceded
# by a space is harmless in YAML, and a rule that flagged it would push authors
# into quoting scalars for no reason.

if [ -z "$(extract desc_truncated_by_yaml_comment)" ]; then
    no "desc_truncated_by_yaml_comment() not found in $SCRIPT — was it renamed?"
else
    # shellcheck disable=SC1090
    eval "$(extract desc_truncated_by_yaml_comment)"
    yaml_bad=""
    # Must flag: a space-preceded # anywhere in the value.
    for s in 'board — the surface for user-owned project #2 "Product Roadmap" and the scope' \
        'closes #823' \
        'trailing #'; do
        desc_truncated_by_yaml_comment "$s" || yaml_bad="$yaml_bad
    missed: $s"
    done
    # Must NOT flag: no space before the #, so YAML keeps the whole scalar.
    for s in 'the C# comparison level' \
        'a tag#value pair' \
        'Explains the board. Use when asked what is on it.' \
        '' ; do
        if desc_truncated_by_yaml_comment "$s"; then
            yaml_bad="$yaml_bad
    false positive: $s"
        fi
    done
    if [ -n "$yaml_bad" ]; then
        no "desc_truncated_by_yaml_comment() does not discriminate" "$yaml_bad"
    else
        ok "desc_truncated_by_yaml_comment() flags ' #' and only ' #'"
    fi
fi

if grep -q 'desc_truncated_by_yaml_comment "\$desc"' "$SCRIPT" &&
    grep -A1 'if desc_truncated_by_yaml_comment "\$desc"; then' "$SCRIPT" | grep -q 'err '; then
    ok "the YAML-comment rule is wired into the lint as an error"
else
    no "the YAML-comment rule is not called from the lint, or no longer calls err" \
        "a predicate nothing invokes is not a gate"
fi

# --- the aws-footprint absence list is a real set difference ---------------
#
# `gen_aws_footprint` derives "services with nothing declared" by matching a
# hand-maintained candidate list against the `resource "aws_*"` types in
# `infra/terraform`. The candidate list is the only input a human edits, so it
# is the only place a typo can hide — and the failure is SILENT, because a
# pattern that matches nothing simply reports its service as absent and the
# drift check sees a self-consistent file.
#
# That is the exact shape of the bug this generator replaced: the skill asserted
# "there is no ... Athena, Glue ... SNS ..." while all three were declared. An
# absence is the one claim the thing it describes cannot falsify, so it needs an
# assertion rather than a reader.
#
# Two checks, neither hard-coding which services exist today:
#   (a) every alternation is a well-formed `aws_*` resource-type prefix
#   (b) the set difference holds BOTH ways — a candidate whose pattern matches a
#       declared type must NOT be on the absence list, and one that matches
#       nothing MUST be
if [ -d infra/terraform ]; then
    shape_bad=""
    logic_bad=""
    declared=$(grep -rhoE 'resource "aws_[a-z0-9_]+"' infra/terraform \
        | sed -e 's/^resource "//' -e 's/"$//' | sort -u || true)
    absence=$(sed -n '/^### Services and resource types with nothing declared/,/^### /p' \
        .claude/skills/aws-account-operations/references/footprint.md \
        | sed -n 's/^- //p' || true)
    candidates=$(sed -n '/local -a candidates=(/,/^\t)/p' "$SCRIPT" | sed -n 's/.*"\(.*\)".*/\1/p')

    while IFS= read -r entry; do
        [ -n "$entry" ] || continue
        label=${entry%%|*}
        pattern=${entry#*|}
        matched=0
        while IFS= read -r alt; do
            [ -n "$alt" ] || continue
            case $alt in
                aws_*) : ;;
                *) shape_bad="$shape_bad $label:'$alt'" ;;
            esac
            hit=$(printf '%s\n' "$declared" | grep -cE "^$alt" || true)
            [ "${hit:-0}" -gt 0 ] && matched=1
        done <<<"$(printf '%s\n' "$pattern" | tr '|' '\n')"

        listed=$(printf '%s\n' "$absence" | grep -cFx "$label" || true)
        if [ "$matched" = 1 ] && [ "${listed:-0}" -gt 0 ]; then
            logic_bad="$logic_bad [$label declared but listed absent]"
        elif [ "$matched" = 0 ] && [ "${listed:-0}" -eq 0 ]; then
            logic_bad="$logic_bad [$label absent but not listed]"
        fi
    done <<<"$candidates"

    if [ -z "$shape_bad" ]; then
        ok "every aws-footprint candidate pattern is a well-formed aws_* prefix"
    else
        no "a candidate pattern is not an aws_* resource-type prefix" "$shape_bad"
    fi

    if [ -z "$logic_bad" ]; then
        ok "the aws-footprint absence list agrees with infra/terraform both ways"
    else
        no "the aws-footprint absence list disagrees with infra/terraform" "$logic_bad"
    fi
else
    ok "no infra/terraform here — aws-footprint absence checks skipped"
fi

# --- the verdict must name what it covered -------------------------------
#
# `regen` asserts nothing, and used to end with the same unconditional
# `skills-check: ok` every checking arm printed. On #812 that read as a pass
# over a SKILL.md eight lines past the body cap — a `lint` error `regen` never
# looks for — and the red landed in CI instead.
#
# THE SLOW HALF IS STUBBED, NOT SKIPPED. `regen` runs 18 generators and was
# measured at 113s on this repo; putting that in a self-test that `make
# skills-check` runs before every gate would be a minute added to every
# invocation. So the real script is copied with ONLY `run_regen` replaced,
# which leaves the dispatcher and the verdict — the logic under test —
# untouched.
#
# The insertion is asserted. Without that, an anchor that stops matching would
# silently run the REAL regen, and the test would pass 113 seconds later having
# proved nothing about the stub it believed it had applied.
stubbed="$tmp/skills-check-stubbed.sh"
awk '
    /^case "\$\{1:-all\}" in$/ { print "run_regen() { note \"regen stubbed by skills-check-test\"; }" }
    { print }
' "$SCRIPT" >"$stubbed"

stub_count=$(grep -cF 'run_regen() { note "regen stubbed by skills-check-test"; }' "$stubbed" || true)
if [ "${stub_count:-0}" -eq 1 ]; then
    ok "the regen stub was inserted exactly once (the anchor still matches)"
else
    no "the regen stub was not inserted — the dispatcher anchor moved" \
        "expected 1 occurrence, found ${stub_count:-0}; the verdict tests below would run the real 113s regen"
fi

if [ "${stub_count:-0}" -eq 1 ]; then
    regen_out=$(bash "$stubbed" regen 2>&1) || true

    # The load-bearing assertion: no bare `ok` verdict from a run that checked
    # nothing. `-Fx` so `ok (overrides)` from a checking arm cannot satisfy it
    # in either direction.
    #
    # Counted into a variable with `|| true` and compared numerically, which is
    # this file's idiom for a reason. The first draft was
    # `… | grep -cFx '…' | grep -qx 0`, and it FAILED against correct output:
    # `grep -c` exits 1 when it counts zero, `pipefail` promotes that to the
    # pipeline's status, and the `if` read it as the bad case — on top of
    # putting `grep -q` behind a pipe, which is the SIGPIPE trap this very file
    # exists to document.
    bare=$(printf '%s\n' "$regen_out" | grep -cFx 'skills-check: ok' || true)
    if [ "${bare:-0}" -eq 0 ]; then
        ok "regen does NOT print a bare 'skills-check: ok'"
    else
        no "regen printed a bare 'skills-check: ok' — a verdict it did not earn" \
            "$(printf '%s' "$regen_out" | tail -3)"
    fi

    if [ "$(printf '%s\n' "$regen_out" | grep -c "make skills-check" || true)" -gt 0 ]; then
        ok "regen names the real gate in its own output"
    else
        no "regen does not name the real gate" "$(printf '%s' "$regen_out" | tail -3)"
    fi

    # And the other direction: an arm that DOES check must still say so, naming
    # itself. `overrides` is the cheapest such arm — no generators, no cargo.
    ovr_out=$(bash "$SCRIPT" overrides 2>&1) || true
    if [ "$(printf '%s\n' "$ovr_out" | grep -c 'skills-check: ok (overrides)' || true)" -gt 0 ]; then
        ok "a checking arm still passes, and the verdict names the stage"
    else
        no "the 'overrides' arm no longer reports 'ok (overrides)'" \
            "$(printf '%s' "$ovr_out" | tail -3)"
    fi
fi

printf '\nskills-check-test: %s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
