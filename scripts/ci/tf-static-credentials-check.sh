#!/usr/bin/env bash
# Keep static AWS credentials out of the Terraform tree.
#
# #549 retired the §9.2 S3 service key, and its acceptance check was a COUNT:
# `aws_iam_access_key` blocks under infra/terraform/ drop from 2 to 1, and the
# survivor is `builder`. A count in a closed issue protects nothing, so this is
# that check as a gate.
#
# TWO RULES, each tied to something that was actually removed:
#
#   1. Every `aws_iam_access_key` resource name must be in ALLOWED. One name is
#      allowed — `builder`, the build-loop key kept indefinitely by owner
#      decision (see builder.tf for its posture; it is prod-guarded by explicit
#      DenyProd* statements). A new static key is not a config tweak: every
#      other AWS identity here is EKS Pod Identity, and a long-lived key is
#      invisible to all of that reasoning and does not expire.
#
#   2. `ER_S3_ACCESS_KEY_ID` / `ER_S3_SECRET_ACCESS_KEY` must not be assigned
#      anywhere under infra/ or scripts/. The engine has no `ER_S3_*` reader at
#      all, so re-plumbing those names cannot make an s3:// lake open — it only
#      recreates a credential nothing consumes. Mentions in COMMENTS are fine
#      and deliberately so: the removal is explained in several of them.
#
# WHY THIS NEEDS ITS OWN SELF-TEST. A detector's job is to notice something, so
# it breaks by noticing nothing — and a parser that silently stops matching
# reports a clean tree forever, which reads exactly like success. This repo has
# been bitten by that shape more than once (the S9 pin gate enforced nothing
# until #467; skills-check truncated and skipped lint). Hence rule 1 carries a
# FLOOR: against the real tree, finding zero access-key blocks is a FAILURE,
# because `builder` is known to be there. Finding none means the regex rotted,
# not that the tree got clean.
#
# Fixture mode: point TF_DIR and/or SHELL_DIRS at a temp tree. The floor is
# skipped when TF_DIR is overridden, so a legitimately empty fixture can be
# asserted as clean.
#
# Usage: bash scripts/ci/tf-static-credentials-check.sh
# Exit:  0 clean; 1 a rule was violated; 2 the gate could not run.

set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2

TF_DIR_DEFAULT=infra/terraform
TF_DIR="${TF_DIR:-$TF_DIR_DEFAULT}"
# Unquoted on purpose below: this is a space-separated list of roots.
SHELL_DIRS="${SHELL_DIRS:-infra scripts}"

# The one permitted static key. Append-only in spirit — adding a name here is
# the reviewable act, which is the whole point of the allowlist being a list.
ALLOWED="builder"

fail=0

note() { printf 'tf-static-credentials: %s\n' "$1"; }
violation() {
	printf 'tf-static-credentials: VIOLATION %s\n' "$1" >&2
	fail=1
}

[ -d "$TF_DIR" ] || {
	note "no such directory: $TF_DIR" >&2
	exit 2
}

# --- Rule 1: the access-key allowlist -------------------------------------
#
# Matches `resource "aws_iam_access_key" "<name>"` and captures <name>. Written
# against the canonical terraform fmt spelling, which `terraform fmt -check`
# (the infra tier) already enforces, so there is no need to tolerate arbitrary
# whitespace here.
keys=$(grep -rhoE '^resource[[:space:]]+"aws_iam_access_key"[[:space:]]+"[^"]+"' \
	--include='*.tf' "$TF_DIR" 2>/dev/null |
	sed -E 's/.*"aws_iam_access_key"[[:space:]]+"([^"]+)".*/\1/' | sort)

# Count without a pipe into the counter: `writer | grep -q` SIGPIPEs the writer
# and a FOUND match can report as missing. Count the captured string instead.
if [ -z "$keys" ]; then
	key_count=0
else
	key_count=$(printf '%s\n' "$keys" | wc -l | tr -d ' ')
fi

if [ "$TF_DIR" = "$TF_DIR_DEFAULT" ] && [ "$key_count" -eq 0 ]; then
	printf 'tf-static-credentials: FAILED TO RUN — zero `aws_iam_access_key` blocks found\n' >&2
	printf '  under %s, but builder.tf is known to declare one. The match pattern has\n' "$TF_DIR" >&2
	printf '  rotted; this is a broken gate reporting a clean tree, not a clean tree.\n' >&2
	exit 2
fi

while IFS= read -r name; do
	[ -n "$name" ] || continue
	case " $ALLOWED " in
	*" $name "*) note "aws_iam_access_key \"$name\" — allowed" ;;
	*) violation "aws_iam_access_key \"$name\" is not in the allowlist ($ALLOWED).
    Every AWS identity here is EKS Pod Identity; a long-lived key does not
    expire and is invisible to that reasoning. If this one is genuinely
    required, add it to ALLOWED in this script with the reason — that edit is
    the review." ;;
	esac
done <<EOF
$keys
EOF

# --- Rule 2: no static object-store credential assignment -----------------
#
# Strip comments first (# for shell/tf, // for tf) so the removal can keep
# being explained in prose. A trailing comment on a real assignment still
# leaves the assignment, so stripping from the marker is safe in that direction.
#
# This gate's own self-test is excluded by name: it carries both forbidden
# spellings as REJECT fixtures, so without the exemption the gate fails on the
# very file that proves it works. Exempting one known path by exact name rather
# than a `*-test.sh` glob keeps the hole as small as possible — the gate script
# itself needs no exemption, since it only ever spells the names as a split
# regex alternation.
SELFTEST=scripts/ci/tf-static-credentials-check-test.sh
for root in $SHELL_DIRS; do
	[ -d "$root" ] || continue
	hits=$(grep -rnE 'ER_S3_(ACCESS_KEY_ID|SECRET_ACCESS_KEY)' \
		--include='*.tf' --include='*.sh' --include='*.yaml' --include='*.yml' --include='*.json' \
		"$root" 2>/dev/null |
		grep -v "^$SELFTEST:" |
		sed -E 's@(#|//).*$@@' |
		grep -E 'ER_S3_(ACCESS_KEY_ID|SECRET_ACCESS_KEY)')
	if [ -n "$hits" ]; then
		violation "a static object-store credential is assigned under $root/:
$(printf '%s\n' "$hits" | sed 's/^/      /')
    The engine has no ER_S3_* reader, so this cannot make an s3:// lake open.
    Opening one is the object-store-config feature the ducklake-lakehouse
    skill's attach-and-tenancy.md tracks, not a credential to re-add."
	fi
done

if [ "$fail" -eq 0 ]; then
	note "clean — $key_count access key(s), all allowed; no static ER_S3_* assignment"
fi

exit "$fail"
