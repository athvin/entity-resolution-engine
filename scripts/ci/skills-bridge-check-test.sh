#!/usr/bin/env bash
# Regression suite for skills-bridge-check.sh — the sibling that forces the
# failure modes every time, same contract as skills-check-test.sh and
# actionlint-required-test.sh: a detector breaks by noticing nothing, and a
# bridge gate that silently stopped iterating reports a clean tree forever.
#
# Every case builds a fixture tree under a temp dir and passes it as the
# check's $1 root. Failure cases must fail with the REASON asserted, not just
# fail — a gate that fails for the wrong reason (e.g. its own typo) teaches
# people to route around it.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
check="$here/skills-bridge-check.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

root="$work/root"
assertions=0

# Fixture: two authored skills, one bridged. $1 names extra bridge entries to
# add (real dir, dangling link, foreign link); the caller edits from there.
mk() {
    rm -rf "$root"
    mkdir -p "$root/.claude/skills/alpha" "$root/.claude/skills/beta"
    printf -- '---\nname: alpha\ndescription: alpha skill\n---\n\nalpha\n' >"$root/.claude/skills/alpha/SKILL.md"
    printf -- '---\nname: beta\ndescription: beta skill\n---\n\nbeta\n' >"$root/.claude/skills/beta/SKILL.md"
    mkdir -p "$root/.agents/skills"
    ln -s ../../.claude/skills/alpha "$root/.agents/skills/alpha"
    for spec in "$@"; do
        case "$spec" in
            beta) ln -s ../../.claude/skills/beta "$root/.agents/skills/beta" ;;
            real-beta)
                mkdir "$root/.agents/skills/beta"
                cp "$root/.claude/skills/beta/SKILL.md" "$root/.agents/skills/beta/SKILL.md"
                ;;
            dangling) ln -s ../../.claude/skills/ghost "$root/.agents/skills/dangling" ;;
            foreign)
                mkdir -p "$root/.agents/skills/.foreign-src"
                ln -sfn .foreign-src "$root/.agents/skills/foreign"
                ;;
        esac
    done
}

expect_pass() {
    local label="$1"
    assertions=$((assertions + 1))
    if out="$(bash "$check" "$root" 2>&1)"; then
        printf 'ok   - %s\n' "$label"
    else
        printf 'BAD  - %s: expected PASS, got:\n%s\n' "$label" "$out" >&2
        exit 1
    fi
}

expect_fail() {
    local label="$1" needle="$2" out
    assertions=$((assertions + 1))
    if out="$(bash "$check" "$root" 2>&1)"; then
        printf 'BAD  - %s: expected FAIL, gate passed anyway\n' "$label" >&2
        exit 1
    fi
    if ! grep -q "$needle" <<<"$out"; then
        printf 'BAD  - %s: failed but not with "%s". Output:\n%s\n' "$label" "$needle" "$out" >&2
        exit 1
    fi
    printf 'ok   - %s (failed with the right reason)\n' "$label"
}

mk beta
expect_pass 'a fully bridged tree'

mk
expect_fail 'a skill without its bridge' 'missing bridge: .agents/skills/beta'

mk real-beta
expect_fail 'a bridge entry replaced by a real copy' 'real directory, not a symlink'

mk dangling
expect_fail 'a dangling bridge symlink' 'dangling symlink'

mk foreign
expect_fail 'a bridge entry with no authored skill' 'no authored skill'

# Resolution mismatch: bridge points at a real but wrong directory.
mk beta
mkdir "$root/.agents/skills/.wrong"
mv "$root/.agents/skills/alpha" "$root/.agents/skills/alpha-ok"
ln -s .wrong "$root/.agents/skills/alpha"
expect_fail 'a bridge symlink that resolves to the wrong directory' 'resolves to'

# Rename detection: the authored tree moved away.
mk beta
mv "$root/.claude/skills" "$root/.claude/skills-moved"
expect_fail 'a renamed .claude/skills' 'is missing (renamed?)'

# The far side moved away — the .agents-only tree is a fork by construction.
mk beta
mv "$root/.agents/skills" "$root/.agents/skills-moved"
expect_fail 'a renamed .agents/skills' 'is missing (renamed?)'

# Mutation guard: if this suite stops discriminating (cases stop building
# fixtures, the check starts passing broken trees), it has asserted nothing
# and must fail, not print a vacuous green.
if [ "$assertions" -eq 0 ]; then
    echo 'BAD  - the suite asserted nothing' >&2
    exit 1
fi
printf 'skills-bridge-check-test: %d cases, all discriminating\n' "$assertions"
