#!/usr/bin/env bash
# Does the alarm path actually terminate at a human?
#
# WHY THIS EXISTS. Every CloudWatch alarm in the account publishes to one SNS
# topic, `er-alerts`, and that topic had exactly one subscription which had
# NEVER BEEN CONFIRMED. Measured 2026-11-17 (#1107): 19 alarms, all
# `ActionsEnabled=True`, publishing to a topic whose only subscriber had not
# accepted delivery. Nothing had ever been delivered, for over a week.
#
# The declaration was correct the whole time. `aws_sns_topic_subscription` is
# present in `envs/core/alerts.tf` and `terraform apply` creates it happily —
# but SNS requires the ADDRESS to confirm, and Terraform cannot do that. An
# unconfirmed subscription is also indistinguishable from a working one in the
# console: both appear in the topic's subscription list. `alerts.tf` says so,
# and the aws-account-operations skill says so, and it still went unnoticed for
# a week, which is why this is now a job rather than a sentence.
#
# WHY NOT LEAVE IT TO `drift`. The drift gate does catch it, but only on the
# wrong side of a three-day delay and for the wrong reason: AWS DELETES a
# subscription that stays pending, the refresh-only plan then reports the
# resource as gone, and `tf-drift-classify.sh` calls it real drift. That is
# a true signal about a deleted resource, arriving days after delivery stopped,
# and it is self-healing in the worst way — the next `apply` re-creates the
# subscription, pending again, so the cycle repeats and the alarms never
# deliver. This checks the property directly instead.
#
# WHAT COUNTS AS BROKEN. Two cases, and only two:
#
#   no subscriptions at all        nothing can receive an alarm
#   no CONFIRMED subscription      every subscriber is still pending, so
#                                  delivery does not happen and AWS will expire
#                                  them
#
# A topic with at least one confirmed subscriber PASSES even if another
# subscription is pending, and says so. That is deliberate: someone adding a
# second destination leaves it pending for a while, and failing the schedule for
# that would be the alarm fatigue this gate is supposed to prevent. The thing
# that matters is whether SOME human receives the alarm.
#
# AWS returns the literal string `PendingConfirmation` in the `SubscriptionArn`
# field for an unconfirmed subscription — it has no ARN yet. That string, not a
# separate status field, is the whole test.
#
# ENDPOINTS ARE REDACTED. A subscription endpoint is an email address or a
# phone number. This runs in a public repository's CI logs, so the local part is
# masked, in the same spirit as the drift scripts never printing attribute
# values.
#
# Usage: alert-path-check.sh <subscriptions-json>
#   where the JSON is the output of
#   `aws sns list-subscriptions-by-topic --topic-arn <arn>`
# Exit codes: 0 the path terminates at a confirmed subscriber; 1 it does not;
# 2 bad usage or unparseable JSON.
set -uo pipefail

die() {
	printf 'alert-path-check: %s\n' "$1" >&2
	exit "${2:-2}"
}

json="${1:-}"
[ -n "$json" ] || die "usage: alert-path-check.sh <subscriptions-json>"
[ -f "$json" ] || die "no such file: $json"
command -v jq >/dev/null 2>&1 || die "jq is required"

# Mask the local part of an email, or all but the last two digits of anything
# else, so the log says which destination without publishing it.
redact() {
	case "$1" in
	*@*) printf '%s***@%s\n' "${1%%"${1#?}"}" "${1#*@}" ;;
	*) printf '***%s\n' "${1: -2}" ;;
	esac
}

# An ABSENT `Subscriptions` key and an EMPTY one mean completely different
# things, and conflating them is how this gate would cry wolf. If AWS ever
# renames the field, `.Subscriptions[]?` yields nothing and the naive version of
# this script reports "the alarm path is dead" — a false incident that sends
# someone hunting a problem that does not exist. So the shape is validated
# first, and an unexpected document is a usage error (2), never a verdict.
jq -e 'has("Subscriptions") and (.Subscriptions | type == "array")' "$json" >/dev/null 2>&1 ||
	die "$json is not an SNS list-subscriptions-by-topic document (no Subscriptions array)"

rows="$(jq -r '.Subscriptions[] | "\(.Protocol)\t\(.Endpoint)\t\(.SubscriptionArn)"' "$json" 2>/dev/null)" ||
	die "could not read the Subscriptions array in $json"

if [ -z "$rows" ]; then
	die "the alarm topic has NO subscriptions — every alarm reaches nobody" 1
fi

confirmed=0
pending=0
pending_list=""
confirmed_list=""

while IFS=$'\t' read -r protocol endpoint arn; do
	[ -n "$protocol" ] || continue
	if [ "$arn" = "PendingConfirmation" ]; then
		pending=$((pending + 1))
		pending_list="$pending_list  $protocol $(redact "$endpoint") — never confirmed"$'\n'
	else
		confirmed=$((confirmed + 1))
		confirmed_list="$confirmed_list  $protocol $(redact "$endpoint")"$'\n'
	fi
done <<EOF
$rows
EOF

if [ "$confirmed" -eq 0 ]; then
	printf 'alert-path-check: THE ALARM PATH IS DEAD — %d subscription(s), none confirmed:\n%s' \
		"$pending" "$pending_list"
	echo 'alert-path-check: an SNS email subscription is confirmed by the ADDRESS, not by'
	echo 'alert-path-check: Terraform. Open the "AWS Notification - Subscription Confirmation"'
	echo 'alert-path-check: message and click Confirm subscription. See #1107.'
	exit 1
fi

printf 'alert-path-check: ok — %d confirmed subscriber(s):\n%s' "$confirmed" "$confirmed_list"
if [ "$pending" -gt 0 ]; then
	printf 'alert-path-check: also %d still pending (not a failure — the path already terminates):\n%s' \
		"$pending" "$pending_list"
fi
exit 0
