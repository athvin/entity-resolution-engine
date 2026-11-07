#!/usr/bin/env bash
# skills-bridge-check — the .agents/skills mirror must stay a symlink bridge.
#
# Claude Code discovers project skills at .claude/skills/; Codex and Kimi Code
# discover them at .agents/skills/. The skills are AUTHORED once, under
# .claude/skills/, and bridged to the other tools through symlinks:
#
#     .agents/skills/<name> -> ../../.claude/skills/<name>
#
# This gate fails the change that:
#   - adds a skill under .claude/skills/ without its bridge (works in Claude,
#     invisible in Codex/Kimi Code), or
#   - lets the bridge fork into a real copy (two skills that drift apart).
#
# Pure bash, no dependencies, no network: a gate with no dependencies cannot
# be deleted by a dependency cleanup (the lesson the S9 pin check documents).
# Runs ungated in rust.yml's `skills` job, because `detect` exempts
# .claude/** — a skills-only PR skips the compile jobs, so a bridge gate
# behind `detect` would never run on exactly the PR it exists for.
#
# Fail-closed on every side:
#   - either tree missing or empty is a failure (rename detection — a gate
#     that examines nothing must not report green);
#   - every .claude/skills/<name> needs a symlink at .agents/skills/<name>
#     that RESOLVES to the authored directory (the link's spelling is not
#     asserted, resolution is, so absolute links still pass);
#   - nothing may live at .agents/skills/ that is not such a symlink, so the
#     bridge can never silently become a forked copy.
#
# Optional $1 is the tree root (defaults to the cwd); the regression suite
# uses it to exercise fixtures outside the repo.
set -euo pipefail

root="${1:-.}"
claude_skills="$root/.claude/skills"
agents_skills="$root/.agents/skills"
fail=0
checked=0

err() { printf 'FAIL: %s\n' "$*" >&2; fail=1; }

# Rename detection, before anything else: both trees must exist.
[ -d "$claude_skills" ] || err "$claude_skills is missing (renamed?)"
[ -d "$agents_skills" ] || err "$agents_skills is missing (renamed?)"
[ "$fail" -eq 0 ] || exit 1

# A glob that matches nothing must expand to nothing, not to itself.
shopt -s nullglob

claude_set=()
for d in "$claude_skills"/*; do
    [ -d "$d" ] || { err "non-directory entry under $claude_skills: $d"; continue; }
    claude_set+=("$(basename "$d")")
done
agents_set=()
for d in "$agents_skills"/*; do
    agents_set+=("$(basename "$d")")
done

# Empty-on-either-side fails: zero skills is not a clean tree, it is a moved
# tree — the zero-match trap the actionlint pin check was found in.
[ "${#claude_set[@]}" -gt 0 ] || err "no skills under $claude_skills — examining nothing"
[ "${#agents_set[@]}" -gt 0 ] || err "no skills under $agents_skills — examining nothing"
[ "$fail" -eq 0 ] || exit 1

# Forward: every authored skill is reachable through a resolving symlink.
for name in "${claude_set[@]}"; do
    checked=$((checked + 1))
    bridge="$agents_skills/$name"
    if [ ! -e "$bridge" ] && [ ! -L "$bridge" ]; then
        err "missing bridge: .agents/skills/$name (Codex/Kimi Code would not see this skill)"
        continue
    fi
    if [ ! -L "$bridge" ]; then
        err ".agents/skills/$name is a real directory, not a symlink — the bridge has forked; point it at ../../.claude/skills/$name"
        continue
    fi
    if [ ! -e "$bridge" ]; then
        err ".agents/skills/$name is a dangling symlink"
        continue
    fi
    resolved="$(cd "$bridge" 2>/dev/null && pwd -P)" || {
        err ".agents/skills/$name does not resolve to a directory"
        continue
    }
    authored="$(cd "$claude_skills/$name" && pwd -P)"
    if [ "$resolved" != "$authored" ]; then
        err ".agents/skills/$name resolves to $resolved, not the authored skill at $authored"
    fi
done

# Reverse: nothing un-bridged lives on the far side — no real copies, no
# skills that exist only for Codex/Kimi and not for Claude.
for name in "${agents_set[@]}"; do
    checked=$((checked + 1))
    bridge="$agents_skills/$name"
    if [ ! -L "$bridge" ]; then
        err ".agents/skills/$name is a real entry, not a symlink"
        continue
    fi
    if [ ! -e "$bridge" ]; then
        err ".agents/skills/$name is a dangling symlink"
        continue
    fi
    case " ${claude_set[*]} " in
        *" $name "*) ;;
        *) err ".agents/skills/$name has no authored skill under $claude_skills" ;;
    esac
done

# Examined-nothing guard: if the loops above ever stop iterating (renamed
# variable, glob change), the count is zero and that fails, not passes.
if [ "$checked" -eq 0 ]; then
    err "examined nothing (rename detection)"
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    printf 'skills-bridge: FAILED (%d checks)\n' "$checked" >&2
    exit 1
fi
printf 'skills-bridge: OK (%d skills bridged)\n' "${#claude_set[@]}"
