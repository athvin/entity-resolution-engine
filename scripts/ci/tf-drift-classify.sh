#!/usr/bin/env bash
# Decide whether a `terraform plan -refresh-only` found REAL drift, or only the
# AWS provider describing things differently than it stored them.
#
# WHY THIS IS NOT JUST AN EXIT CODE. The first real run of `drift.yml` used
# `plan -refresh-only -detailed-exitcode` and treated 2 as drift. Measured
# result across four stacks: `dev` and `prod` both printed "found no
# differences" and both exited 2. A refresh-only plan has something to persist
# almost by construction, so that exit code is not a drift signal — it would
# have turned the schedule red every day, which is the alarm fatigue the gate
# was supposed to avoid.
#
# WHY IT IS NOT THE TEXT SENTINEL EITHER. "Objects have changed outside of
# Terraform" fired on two stacks, and neither was drift:
#
#   state-backend  `tags = {}` appearing where state holds null, and
#                  `aws_s3_bucket.policy` reporting the policy that a SEPARATE
#                  `aws_s3_bucket_policy` resource manages.
#   core           `managed_policy_arns` reporting the attachment that a
#                  SEPARATE `aws_iam_role_policy_attachment` manages.
#
# Both are the same shape: an attribute the provider computes from a resource
# Terraform manages elsewhere, or a null/empty normalisation. Neither means
# anyone touched the account. And `state-backend` is PLAN-ONLY — CI never
# applies it — so nothing would ever reconcile its state snapshot and it would
# report drift forever.
#
# The second run (41283281935) cleared `dev` and `prod` and surfaced three more
# of the same class that the truncated text output had hidden:
# `aws_s3_bucket.server_side_encryption_configuration`, `.versioning` and
# `aws_iam_role.inline_policy`. That is what turned the ignore list from a set
# of observations into a rule.
#
# So drift is decided per ATTRIBUTE, from the plan JSON. Add to the lists below
# only on one of two justifications — the sibling-resource rule stated there, or
# a real run proving the attribute is normalisation noise. Never on a hunch:
# every entry without one of those is a place real drift can hide.
#
# NOTHING FROM THE JSON IS PRINTED EXCEPT ADDRESSES AND ATTRIBUTE NAMES. A plan
# JSON carries attribute VALUES in cleartext, which is why the apply tier is
# forbidden from printing `show -json` at all. Here the jq below emits only
# `.address` and the NAMES of attributes that differ — never a value, never the
# raw document.
#
# Usage: tf-drift-classify.sh <plan-json>
# Exit codes: 0 no real drift; 1 real drift (addresses printed); 2 bad usage or
# unparseable JSON.
set -uo pipefail

# Ignored on ANY type: the null-versus-empty normalisation the provider applies
# to every taggable resource. Observed on three resource types in run 1.
IGNORE_ANY="tags tags_all"

# Ignored on ONE type, by the SIBLING-RESOURCE RULE: on that type the provider
# deprecated the attribute in favour of a separate resource, and now computes it
# from whatever that resource set.
#
# This is the one rule here that is principled rather than merely observed, and
# why it hides nothing is the whole point: the sibling resource Terraform
# actually manages reports the same drift itself. Turn bucket versioning off in
# the console and `aws_s3_bucket_versioning` drifts — the deprecated
# `aws_s3_bucket.versioning` mirror saying so too is duplicate noise, not the
# signal.
#
# That rule is also the boundary. An attribute belongs here only when a sibling
# resource owns it on that type: `policy` is ignored on `aws_s3_bucket` (owned
# by `aws_s3_bucket_policy`) and NOWHERE else, because a real change to an
# `aws_iam_role_policy` document must stay loud. There is a test for exactly
# that.
IGNORE_ON_TYPE="
aws_s3_bucket:policy
aws_s3_bucket:acl
aws_s3_bucket:grant
aws_s3_bucket:versioning
aws_s3_bucket:server_side_encryption_configuration
aws_s3_bucket:lifecycle_rule
aws_s3_bucket:logging
aws_s3_bucket:cors_rule
aws_s3_bucket:website
aws_s3_bucket:replication_configuration
aws_s3_bucket:object_lock_configuration
aws_s3_bucket:request_payer
aws_iam_role:managed_policy_arns
aws_iam_role:inline_policy
"

die() {
    printf 'tf-drift-classify: %s\n' "$1" >&2
    exit "${2:-2}"
}

json="${1:-}"
[ -n "$json" ] || die "usage: tf-drift-classify.sh <plan-json>"
[ -f "$json" ] || die "no such file: $json"
command -v jq >/dev/null 2>&1 || die "jq is required"

# Per drifted resource: its address, its type, and the NAMES of the attributes
# whose values differ between before and after. Values never leave jq.
extracted="$(
    jq -r '
      .resource_drift[]? as $r
      | (($r.change.before // {})) as $b
      | (($r.change.after  // {})) as $a
      | ((($b | keys) + ($a | keys)) | unique | map(select($b[.] != $a[.]))) as $diff
      | "\($r.address)\t\($r.type)\t\($diff | join(","))"
    ' "$json" 2>/dev/null
)" || die "could not parse $json as a terraform plan JSON document"

if [ -z "$extracted" ]; then
    echo "tf-drift-classify: no resource_drift entries — state matches reality"
    exit 0
fi

real=""
ignored=""

while IFS=$'\t' read -r addr rtype diff; do
    [ -n "$addr" ] || continue
    # An entry with no differing attributes is noise by definition.
    if [ -z "$diff" ]; then
        ignored="$ignored  $addr (no differing attributes)"$'\n'
        continue
    fi

    # Commas to spaces rather than `IFS=','`, which is what the first draft did
    # and why the ignore list did nothing: with IFS set to a comma, the
    # space-separated `$IGNORE_ANY` stopped splitting and became one long word,
    # so no attribute ever equalled an entry. Terraform attribute names are HCL
    # identifiers and cannot contain spaces, so the default IFS is safe here.
    remaining=""
    for attr in $(printf '%s' "$diff" | tr ',' ' '); do
        skip=0
        for ig in $IGNORE_ANY; do
            [ "$attr" = "$ig" ] && skip=1
        done
        for pair in $IGNORE_ON_TYPE; do
            [ "$rtype:$attr" = "$pair" ] && skip=1
        done
        [ "$skip" -eq 1 ] || remaining="$remaining,$attr"
    done

    remaining="${remaining#,}"
    if [ -z "$remaining" ]; then
        ignored="$ignored  $addr [$diff]"$'\n'
    else
        real="$real  $addr [$remaining]"$'\n'
    fi
done <<EOF
$extracted
EOF

if [ -n "$ignored" ]; then
    printf 'tf-drift-classify: ignored as provider artifacts:\n%s' "$ignored"
fi

if [ -z "$real" ]; then
    echo "tf-drift-classify: no real drift"
    exit 0
fi

printf 'tf-drift-classify: REAL DRIFT — these changed outside Terraform:\n%s' "$real"
exit 1
