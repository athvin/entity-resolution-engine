#!/usr/bin/env bash
# Reduce one `terraform plan` run to the handful of facts the PR comment and the
# run summary need, as `key=value` lines.
#
# This is a separate file from the workflow for the same reason its two siblings
# are: a parser embedded in a matrix job can only be exercised by pushing a
# commit, and this one decides what a reviewer is told. Getting `destroy=0` out
# of a plan that destroys something is the worst available failure, so it is
# worth a fixture rather than a heredoc.
#
# THE EXIT CODE IS THE AUTHORITY ON WHETHER ANYTHING CHANGED, not the text.
# `-detailed-exitcode` distinguishes 0 = no changes, 2 = changes, 1 = error —
# without it a plan that errored and a plan that was empty share an exit code,
# which is precisely how a broken plan gets reported as "nothing to do".
#
# Usage:
#   tf-plan-meta.sh <plan-output> <terraform-exit-code>  > meta.txt
#
# Emits:
#   status=ok|error   whether the plan ran at all
#   changes=true|false
#   add=N change=N destroy=N   blank when status=error, or when Terraform
#                              printed no `Plan:` summary (an outputs-only diff)
set -uo pipefail

plan_file="${1:-}"
code="${2:-}"

if [ -z "$plan_file" ] || [ -z "$code" ]; then
    printf 'tf-plan-meta: usage: tf-plan-meta.sh <plan-output> <terraform-exit-code>\n' >&2
    exit 2
fi
if [ ! -f "$plan_file" ]; then
    printf 'tf-plan-meta: no such file: %s\n' "$plan_file" >&2
    exit 2
fi

case "$code" in
0) status=ok changes=false ;;
2) status=ok changes=true ;;
*) status=error changes=false ;;
esac

add="" change="" destroy=""
if [ "$status" = "ok" ]; then
    # Each count is read from its own clause rather than from one pattern over
    # the whole line: with an import block Terraform prints
    # `Plan: 1 to import, 0 to add, …`, and a single full-line pattern would
    # match nothing there and silently report blanks.
    summary="$(grep -E '^Plan: ' "$plan_file" | tail -1)"
    if [ -n "$summary" ]; then
        add="$(printf '%s' "$summary" | sed -nE 's/.*[^0-9]([0-9]+) to add.*/\1/p')"
        change="$(printf '%s' "$summary" | sed -nE 's/.*[^0-9]([0-9]+) to change.*/\1/p')"
        destroy="$(printf '%s' "$summary" | sed -nE 's/.*[^0-9]([0-9]+) to destroy.*/\1/p')"
    elif grep -qE '^No changes\.' "$plan_file"; then
        add=0 change=0 destroy=0
    fi
fi

printf 'status=%s\n' "$status"
printf 'changes=%s\n' "$changes"
printf 'add=%s\n' "$add"
printf 'change=%s\n' "$change"
printf 'destroy=%s\n' "$destroy"
