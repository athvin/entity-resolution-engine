#!/usr/bin/env bash
# Assemble the single sticky `terraform plan` comment a PR carries, from the
# per-env artifacts the plan matrix uploaded.
#
# WHY A SCRIPT AND NOT INLINE YAML. Two reasons, both learned here. A gate is
# not trusted until it has been observed failing (.claude/skills/frontend-testing/references/frontend-testing.md), and
# a heredoc inside a matrix job can only be exercised by pushing a commit; this
# runs against fixture plan files in a second. And the character budget below is
# real arithmetic — GitHub rejects a comment body over 65536 characters outright,
# so a plan large enough to breach it would otherwise turn the review surface
# off at exactly the moment it matters most.
#
# ONE comment, not one per env: the matrix legs finish at different times, and
# three independently-refreshing comments on every push is a PR nobody reads.
# The caller upserts it by the `<!-- terraform-plan -->` marker this emits.
#
# Usage:
#   tf-plan-comment.sh <artifacts-dir>  > body.md
#
# Expects, per environment key, a directory named `plan-<key>` holding:
#   plan.txt   the `terraform plan -no-color` output, verbatim
#   meta.txt   key=value lines: status, add, change, destroy, changes
#
# Exit codes: 0 a body was written; 2 nothing to report (no artifacts — which is
# what a `detect`-skipped run looks like, and the caller should post nothing).
set -uo pipefail

# Display order, independent of however the artifacts were downloaded. Anything
# not listed here is appended rather than dropped: a future environment must
# show up unprompted, because a plan table that silently omits an env is worse
# than an ugly one.
ORDER="state-backend core dev prod"

# Stacks the apply tier deliberately does not carry. Marked in the table because
# without it a row reads as "merging applies this", which for `state-backend` is
# exactly wrong — it is planned for drift detection and never applied, since it
# owns the bucket its own backend lives in. Keep in step with the apply matrix.
PLAN_ONLY="state-backend"

# 65536 is the hard cap on a comment body. The rest of the body — table, header,
# notes, the `<details>` scaffolding — measures a few hundred characters per
# env, so reserving ~7k for it leaves this for the plan text itself, split
# evenly across however many envs actually have something to show.
BODY_BUDGET=58000

dir="${1:-}"
[ -n "$dir" ] || {
    printf 'tf-plan-comment: usage: tf-plan-comment.sh <artifacts-dir>\n' >&2
    exit 2
}
[ -d "$dir" ] || {
    printf 'tf-plan-comment: no such directory: %s\n' "$dir" >&2
    exit 2
}

meta_get() {
    # $1 file, $2 key. Read rather than source: this is generated data, and
    # sourcing it would execute whatever ended up in a plan-derived value.
    sed -nE "s/^$2=(.*)$/\1/p" "$1" | tail -1
}

env_label() {
    case " $PLAN_ONLY " in
    *" $1 "*) printf '`%s` _(plan only)_' "$1" ;;
    *) printf '`%s`' "$1" ;;
    esac
}

# --- which envs do we have, in which order ----------------------------------

keys=""
for key in $ORDER; do
    [ -d "$dir/plan-$key" ] && keys="$keys $key"
done
# bash 3.2 ships on macOS and has no `mapfile`, so this is a read loop.
while IFS= read -r found; do
    [ -n "$found" ] || continue
    case " $keys " in
    *" $found "*) ;;
    *) keys="$keys $found" ;;
    esac
done < <(find "$dir" -maxdepth 1 -type d -name 'plan-*' -exec basename {} \; 2>/dev/null | sed 's/^plan-//' | sort)

# shellcheck disable=SC2086
set -- $keys
[ "$#" -gt 0 ] || {
    printf 'tf-plan-comment: no plan-* artifacts under %s — nothing to report\n' "$dir" >&2
    exit 2
}

# --- how much plan text may each env print ----------------------------------

detailed=0
for key in "$@"; do
    meta="$dir/plan-$key/meta.txt"
    [ -f "$meta" ] || continue
    status="$(meta_get "$meta" status)"
    changes="$(meta_get "$meta" changes)"
    if [ "$status" != "ok" ] || [ "$changes" = "true" ]; then
        detailed=$((detailed + 1))
    fi
done
per_env=$BODY_BUDGET
[ "$detailed" -gt 0 ] && per_env=$((BODY_BUDGET / detailed))

# --- header and table -------------------------------------------------------

printf '### terraform plan\n\n'
if [ -n "${GITHUB_SHA:-}" ]; then
    short="$(printf '%s' "$GITHUB_SHA" | cut -c1-7)"
    if [ -n "${GITHUB_SERVER_URL:-}" ] && [ -n "${GITHUB_REPOSITORY:-}" ] && [ -n "${GITHUB_RUN_ID:-}" ]; then
        printf '`%s` · [run log and full plans](%s/%s/actions/runs/%s)\n\n' \
            "$short" "$GITHUB_SERVER_URL" "$GITHUB_REPOSITORY" "$GITHUB_RUN_ID"
    else
        printf '`%s`\n\n' "$short"
    fi
fi

printf '| env | add | change | destroy |\n'
printf '|---|---|---|---|\n'
for key in "$@"; do
    meta="$dir/plan-$key/meta.txt"
    label="$(env_label "$key")"
    if [ ! -f "$meta" ]; then
        printf '| %s (no metadata) | ? | ? | ? |\n' "$label"
        continue
    fi
    status="$(meta_get "$meta" status)"
    if [ "$status" != "ok" ]; then
        printf '| %s — **plan failed** | – | – | – |\n' "$label"
        continue
    fi
    add="$(meta_get "$meta" add)"
    change="$(meta_get "$meta" change)"
    destroy="$(meta_get "$meta" destroy)"
    # The destroy cell is the one a reviewer must not scroll past: on an
    # unattended apply it is the only place a teardown is visible while it is
    # still reversible.
    if [ "${destroy:-0}" != "0" ]; then
        printf '| %s | %s | %s | **%s** |\n' "$label" "${add:-0}" "${change:-0}" "$destroy"
    else
        printf '| %s | %s | %s | %s |\n' "$label" "${add:-0}" "${change:-0}" "${destroy:-0}"
    fi
done

printf '\n> Planned with `-refresh=false`, so this is the **config-versus-state** diff and\n'
printf '> not out-of-band drift. That is a security choice, not a speed one: a refreshing\n'
printf '> plan reads secret *values*, and the plan role is assumable from any PR branch —\n'
printf '> which can edit the workflow it runs under. Drift is caught at apply time, which\n'
printf '> refreshes fully.\n'

# Only stacks the apply tier actually carries belong in this warning: its whole
# claim is "merging applies this", which is false for a plan-only stack. Their
# destroy count is still bolded in the table above, because config and state
# disagreeing is worth seeing either way — it just is not about to happen.
destroying=""
for key in "$@"; do
    meta="$dir/plan-$key/meta.txt"
    [ -f "$meta" ] || continue
    [ "$(meta_get "$meta" status)" = "ok" ] || continue
    case " $PLAN_ONLY " in *" $key "*) continue ;; esac
    [ "$(meta_get "$meta" destroy)" != "0" ] && destroying="$destroying $key"
done
if [ -n "$destroying" ]; then
    printf '\n> [!WARNING]\n'
    printf '> This plan DESTROYS resources in:%s. Merging applies it unattended.\n' "$destroying"
    printf '> The apply refuses to run unless the **merge commit message** carries\n'
    printf '> `ALLOW-DESTROY`. Retitling is not enough on a single-commit PR — GitHub\n'
    printf '> uses the commit message there, not the title. Merge with\n'
    printf '> `gh pr merge <n> --squash --subject '"'"'<subject> ALLOW-DESTROY'"'"'`.\n'
fi

# --- per-env detail ---------------------------------------------------------

for key in "$@"; do
    meta="$dir/plan-$key/meta.txt"
    plan="$dir/plan-$key/plan.txt"
    [ -f "$meta" ] || continue

    status="$(meta_get "$meta" status)"
    changes="$(meta_get "$meta" changes)"
    add="$(meta_get "$meta" add)"
    change="$(meta_get "$meta" change)"
    destroy="$(meta_get "$meta" destroy)"

    # No block for a clean env. On the common PR — one that touches no
    # Terraform at all — that leaves just the table, which is the whole report.
    if [ "$status" = "ok" ] && [ "$changes" != "true" ]; then
        continue
    fi

    if [ "$status" != "ok" ]; then
        summary="$key — plan FAILED"
        open=" open"
    elif [ "${destroy:-0}" != "0" ]; then
        summary="$key — ${add:-0} to add, ${change:-0} to change, ${destroy} to DESTROY"
        open=" open"
    else
        summary="$key — ${add:-0} to add, ${change:-0} to change"
        open=""
    fi

    printf '\n<details%s><summary>%s</summary>\n\n' "$open" "$summary"

    if [ ! -f "$plan" ]; then
        printf 'No plan output was uploaded for this environment.\n'
        printf '\n</details>\n'
        continue
    fi

    size="$(wc -c <"$plan" | tr -d ' ')"
    # Four tildes, not three backticks: plan output can legitimately contain a
    # triple-backtick inside a string value, which would close a backtick fence
    # early and spill raw plan text into the comment as markdown.
    if [ "$size" -gt "$per_env" ]; then
        printf '_Showing the last %s of %s characters — the full plan is in the run summary._\n\n' \
            "$per_env" "$size"
        printf '~~~~terraform\n'
        tail -c "$per_env" "$plan"
        printf '\n~~~~\n'
    else
        printf '~~~~terraform\n'
        cat "$plan"
        printf '\n~~~~\n'
    fi
    printf '\n</details>\n'
done

# The upsert key. Everything above may change freely; this line is the identity
# of the comment and must not.
printf '\n<!-- terraform-plan -->\n'
