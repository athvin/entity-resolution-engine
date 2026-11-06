#!/usr/bin/env bash
# Refuse an unattended `terraform apply` that would destroy anything, unless the
# merge commit says so out loud.
#
# WHY THIS EXISTS. Applies are unattended (infrastructure.md §14.2): a merge to
# `main` is the trigger and the only trigger, and §14.2's prod-scoped apply role
# — the second, separately auditable hop — is not built. Prod is provisioned
# now: an Aurora cluster with two instances, EFS, EKS. So the gap between "a
# reviewer skimmed a plan comment" and "production is gone" is one merge, with
# nothing in between.
#
# This is a SPEED BUMP, not a policy engine, and the difference matters. It
# counts destroys; it cannot tell a benign replace of a tag-only resource from
# losing a database. A *replace* reads as `1 to add, 1 to destroy`, so routine
# replacements trip it too. That is the intended trade — uniform across every
# applied env, with no per-env carve-out to forget about.
#
# THE OPT-IN is the token `ALLOW-DESTROY` in the merge commit message, which is
# what this reads. Stating it in prose makes the intent reviewable in the same
# diff as the destroy itself, which a workflow input or a label would not.
#
# GETTING IT THERE IS THE PART THAT BITES, and this comment used to say "put it
# in the PR title" as though that were sufficient. It is not. GitHub composes a
# squash message from the PR title and body only when the PR has **two or more**
# commits; for a SINGLE-commit PR it uses that commit's own message and ignores
# the title entirely. A small infra PR is usually one commit, so the advice was
# wrong in exactly the common case — #509 was retitled to carry the token,
# merged, and the apply refused anyway, because the lone commit's message
# predated the retitle.
#
# So, reliably, either:
#   * put the token in the COMMIT message (amend and force-push), or
#   * pass it explicitly at merge time:
#       gh pr merge <n> --squash --subject '<subject> ALLOW-DESTROY'
#
# A multi-commit PR does take its title, so that advice was not wrong
# everywhere — only unreliable, which is worse.
#
# IT FAILS CLOSED ON UNPARSEABLE INPUT, deliberately. The recurring bug class in
# this repository is a gate that passes because it never actually looked —
# semgrep reporting 0 findings on a parse error, a Playwright exit code
# swallowed by `| tail`. A guard that reads "no destroys" out of a truncated or
# unexpected file is that same bug with a bigger blast radius, so anything it
# cannot positively classify is an error.
#
# Usage:
#   tf-destroy-guard.sh <show-output> [commit-message-file]
#
#   <show-output>          `terraform show -no-color <planfile>` output. NOT the
#                          planfile itself and never `show -json`: a saved plan
#                          carries sensitive values in cleartext, while this
#                          human form redacts them to `(sensitive value)`.
#   [commit-message-file]  Overrides `git log -1` — this is how the guard is
#                          exercised against fixtures without inventing commits.
#
# Exit codes: 0 allowed (no destroys, or destroys with the token); 1 refused;
# 2 bad usage or unparseable input.
set -uo pipefail

TOKEN="ALLOW-DESTROY"

die() {
    printf 'tf-destroy-guard: %s\n' "$1" >&2
    exit "${2:-2}"
}

plan_file="${1:-}"
[ -n "$plan_file" ] || die "usage: tf-destroy-guard.sh <show-output> [commit-message-file]"
[ -f "$plan_file" ] || die "no such file: $plan_file"

# The destroy count, anchored on its own clause rather than the whole line: with
# an import block Terraform prints `Plan: 1 to import, 0 to add, …`, so a
# pattern matching the full summary would silently match nothing there and fall
# through to the unparseable branch.
destroy="$(sed -nE 's/^Plan: .*, ([0-9]+) to destroy\.$/\1/p' "$plan_file" | tail -1)"

if [ -n "$destroy" ]; then
    basis="plan summary"
elif grep -qE '^No changes\.' "$plan_file"; then
    # An empty plan. Every merge to `main` that touches no Terraform lands here.
    destroy=0
    basis="no changes"
elif grep -qE '^Changes to Outputs:' "$plan_file"; then
    # Output-only diffs print no `Plan:` line at all. A real state, not garbage.
    destroy=0
    basis="outputs only"
else
    printf 'tf-destroy-guard: cannot classify %s — no `Plan:` summary, no `No changes.`,\n' "$plan_file" >&2
    printf 'tf-destroy-guard: no `Changes to Outputs:`. Refusing rather than assuming zero\n' >&2
    printf 'tf-destroy-guard: destroys. First and last 20 lines:\n' >&2
    head -20 "$plan_file" >&2
    printf 'tf-destroy-guard: ---\n' >&2
    tail -20 "$plan_file" >&2
    exit 2
fi

if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf 'destroy=%s\n' "$destroy" >>"$GITHUB_OUTPUT"
fi

if [ "$destroy" -eq 0 ]; then
    printf 'tf-destroy-guard: 0 to destroy (%s) — allowed\n' "$basis"
    exit 0
fi

# Destroys present. The only remaining question is whether anyone said so.
if [ -n "${2:-}" ]; then
    [ -f "$2" ] || die "no such commit-message file: $2"
    message="$(cat "$2")"
else
    message="$(git log -1 --pretty=%B 2>/dev/null)" || die "git log failed and no commit-message file was given"
    [ -n "$message" ] || die "git log returned an empty commit message"
fi

case "$message" in
*"$TOKEN"*)
    printf 'tf-destroy-guard: %s to destroy, and the commit message carries %s — allowed\n' "$destroy" "$TOKEN"
    exit 0
    ;;
esac

if [ -n "${GITHUB_ACTIONS:-}" ]; then
    printf '::error::terraform apply refused: %s resource(s) would be DESTROYED and the merge commit does not carry %s\n' \
        "$destroy" "$TOKEN"
fi
printf 'tf-destroy-guard: REFUSED. This apply would destroy %s resource(s).\n' "$destroy" >&2
printf 'tf-destroy-guard: Nothing has been applied. To allow it, the MERGE COMMIT message\n' >&2
printf 'tf-destroy-guard: must contain the token %s.\n' "$TOKEN" >&2
printf 'tf-destroy-guard:\n' >&2
printf 'tf-destroy-guard: Retitling the PR is NOT enough on its own: GitHub only builds the\n' >&2
printf 'tf-destroy-guard: squash message from the PR title when the PR has two or more\n' >&2
printf 'tf-destroy-guard: commits. With a single commit it uses that commit message. Either\n' >&2
printf 'tf-destroy-guard: amend the commit, or merge with an explicit subject:\n' >&2
printf 'tf-destroy-guard:   gh pr merge <n> --squash --subject '"'"'<subject> %s'"'"'\n' "$TOKEN" >&2
exit 1
