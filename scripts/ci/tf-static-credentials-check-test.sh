#!/usr/bin/env bash
# Self-test for tf-static-credentials-check.sh.
#
# The gate's job is to notice a static credential, so the way it breaks is by
# noticing nothing — and that failure reads exactly like a clean tree. So each
# rule is asserted in BOTH directions: a fixture it must reject, and a fixture
# it must accept. A gate that accepts everything fails the reject cases; a gate
# that rejects everything fails the accept cases.
#
# The third block is the one that matters most: it proves the FLOOR fires, i.e.
# that a run finding zero access-key blocks against the real tree exits 2
# rather than reporting success.
#
# Fixtures go in a temp dir and the gate is pointed at them with TF_DIR /
# SHELL_DIRS, so nothing here touches infra/ or scripts/.
#
# Run: bash scripts/ci/tf-static-credentials-check-test.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2

GATE=scripts/ci/tf-static-credentials-check.sh
pass=0
fail=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# case_ <name> <want-exit> <tf-body> <shell-body>
case_() {
	local name=$1 want=$2 tf=$3 sh=$4
	local d="$tmp/c$((pass + fail))"
	mkdir -p "$d/tf" "$d/sh"
	printf '%s\n' "$tf" >"$d/tf/main.tf"
	printf '%s\n' "$sh" >"$d/sh/thing.sh"
	local got out
	# Both fixture dirs go to SHELL_DIRS, because rule 2 must see terraform
	# too: in the real invocation SHELL_DIRS is "infra scripts" and the
	# terraform tree lives UNDER infra/, so a tf-only fixture outside the
	# shell roots would quietly assert nothing.
	out=$(TF_DIR="$d/tf" SHELL_DIRS="$d/tf $d/sh" bash "$GATE" 2>&1)
	got=$?
	if [ "$got" = "$want" ]; then
		pass=$((pass + 1))
		printf 'ok   %s\n' "$name"
	else
		fail=$((fail + 1))
		printf 'FAIL %s\n       want exit: %s\n        got exit: %s\n        output: %s\n' \
			"$name" "$want" "$got" "$out"
	fi
}

# --- Rule 1: the access-key allowlist -------------------------------------

case_ 'rejects a non-allowlisted access key' 1 \
	'resource "aws_iam_access_key" "s3" {
  user = aws_iam_user.s3.name
}' ''

case_ 'accepts the allowlisted builder key' 0 \
	'resource "aws_iam_access_key" "builder" {
  user = aws_iam_user.builder.name
}' ''

case_ 'rejects a second key even beside the allowed one' 1 \
	'resource "aws_iam_access_key" "builder" {
  user = aws_iam_user.builder.name
}

resource "aws_iam_access_key" "lake" {
  user = aws_iam_user.lake.name
}' ''

case_ 'accepts a tree with no keys at all (fixture mode)' 0 \
	'resource "aws_s3_bucket" "lake" {
  bucket = "er-dev-lake"
}' ''

# --- Rule 2: no static object-store credential ----------------------------

case_ 'rejects an ER_S3_ACCESS_KEY_ID assignment in shell' 1 \
	'' 'export ER_S3_ACCESS_KEY_ID="AKIAEXAMPLE"'

case_ 'rejects an ER_S3_SECRET_ACCESS_KEY assignment in terraform' 1 \
	'resource "aws_secretsmanager_secret_version" "s3" {
  secret_string = jsonencode({
    ER_S3_SECRET_ACCESS_KEY = aws_iam_access_key.builder.secret
  })
}' ''

case_ 'accepts the names when they appear only in a comment' 0 \
	'# ER_S3_ACCESS_KEY_ID was retired by 4.1; nothing reads it.' \
	'# the ER_S3_SECRET_ACCESS_KEY pair is gone — see lake.tf'

# --- The floor: a parser that notices nothing must not report success -----
#
# Driven against the REAL default TF_DIR, where builder.tf is known to declare
# a key. The floor only fires when TF_DIR is the default, so this case cannot
# be expressed through case_ above.
floor_out=$(SHELL_DIRS="$tmp/empty" bash "$GATE" 2>&1)
floor_got=$?
if [ "$floor_got" = 0 ]; then
	pass=$((pass + 1))
	printf 'ok   real tree is clean, and the floor found the builder key\n'
else
	fail=$((fail + 1))
	printf 'FAIL real tree should be clean\n        got exit: %s\n        output: %s\n' \
		"$floor_got" "$floor_out"
fi

# Now prove the floor itself: an empty default-named tree must exit 2, not 0.
# Staged by making a temp dir that IS the default path, under a temp cwd.
floor_dir="$tmp/floorcheck"
mkdir -p "$floor_dir/infra/terraform" "$floor_dir/scripts/ci"
cp "$GATE" "$floor_dir/scripts/ci/"
printf 'resource "aws_s3_bucket" "b" {\n  bucket = "x"\n}\n' \
	>"$floor_dir/infra/terraform/main.tf"
floor2_out=$(cd "$floor_dir" && SHELL_DIRS="nope" bash scripts/ci/tf-static-credentials-check.sh 2>&1)
floor2_got=$?
if [ "$floor2_got" = 2 ]; then
	pass=$((pass + 1))
	printf 'ok   zero access keys under the DEFAULT dir exits 2, not 0\n'
else
	fail=$((fail + 1))
	printf 'FAIL the floor did not fire — a gate that notices nothing reported success\n'
	printf '        want exit: 2\n        got exit: %s\n        output: %s\n' \
		"$floor2_got" "$floor2_out"
fi

# --- A suite that asserted nothing is not a passing suite -----------------
if [ "$((pass + fail))" -lt 9 ]; then
	printf 'FAIL the suite ran %s cases; it should run at least 9. Did a case stop executing?\n' \
		"$((pass + fail))"
	fail=$((fail + 1))
fi

printf '\ntf-static-credentials-check-test: %s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
