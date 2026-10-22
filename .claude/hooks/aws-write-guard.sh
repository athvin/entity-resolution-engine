#!/usr/bin/env bash
# Layer 3 of the AWS guardrails: the Bash bypass.
#
# The IAM condition keys (aws:ViaAWSMCPService) exist ONLY on requests routed
# through an AWS-managed MCP server. `aws s3 rm`, `terraform apply` and
# `kubectl delete` run under the same SSO credentials and are untouched by them,
# so without this hook the server-side deny is a guardrail with a door next to
# it. See .claude/skills/aws-account-operations/references/mcp-and-guardrails.md
# for what each of the three layers misses.
#
# Decision rule, in order:
#   1. reads pass silently (exit 0, no output)
#   2. unambiguously destructive calls are DENIED
#   3. everything else mutating escalates to ASK, so a human decides and the
#      workflow survives
#
# `ask` rather than `deny` for the middle case is deliberate. A guard that
# blocks legitimate work gets disabled, and a disabled guard protects nothing.
set -uo pipefail

payload=$(cat)
command=$(printf '%s' "$payload" | jq -r '.tool_input.command // ""')

# Nothing to inspect — let the normal permission flow handle it.
[[ -n $command ]] || exit 0

# Strip leading environment assignments so `AWS_PROFILE=x aws …` is matched as
# an `aws` command. Without this the generic sweep anchors on `^aws` and every
# guarded call escapes simply by being prefixed with a variable — which is also
# the exact shape the er-builder credential is used in.
bare=$command
while [[ $bare =~ ^[[:space:]]*[A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+(.*)$ ]]; do
	bare=${BASH_REMATCH[1]}
done

decide() {
	# $1 = deny|ask, $2 = reason
	jq -n --arg d "$1" --arg r "$2" '{
		hookSpecificOutput: {
			hookEventName: "PreToolUse",
			permissionDecision: $d,
			permissionDecisionReason: $r
		}
	}'
	exit 0
}

# --- the known-good write paths already in the tree ------------------------
# These are named exceptions rather than collateral damage. Each is a write
# that the repo's own tooling performs as a matter of course; blocking them
# would break `scripts/dev_env.sh` and `make image-api`.
case $command in
*"aws ecr get-login-password"*)
	exit 0
	;;
*dev_env.sh*)
	# The script's own secretsmanager + s3 rm calls, invoked as a unit.
	exit 0
	;;
esac

# --- the one identity no other layer constrains ----------------------------
# er-builder is backed by a never-expiring AdministratorAccess static key. The
# MCP condition keys cannot reach it (they apply to permission sets, not IAM
# user keys), so here the IDENTITY is the trigger, not the verb.
if [[ $command == *"AWS_PROFILE=er-builder"* || $command == *"--profile er-builder"* ]]; then
	# Anything that is not plainly a read. Enumerating mutating verbs here would
	# be the wrong way round: this credential is unconstrained by every other
	# layer, so the default must be to ask and reads are the exception.
	if ! [[ $bare =~ [[:space:]](describe|list|get|ls|head|wait|logs|version)[a-z-]*([[:space:]]|$) ]]; then
		decide ask "er-builder is the never-expiring AdministratorAccess key, and no guardrail layer constrains it — not the MCP condition keys, which apply to permission sets rather than IAM user keys. Confirm this deliberately."
	fi
fi

# --- unambiguously destructive ---------------------------------------------
# Deliberately narrow: each of these destroys state that is either production
# or expensive to rebuild, and none has a legitimate agent-initiated use.
destructive_patterns=(
	'aws[[:space:]]+eks[[:space:]]+delete-cluster'
	'aws[[:space:]]+rds[[:space:]]+delete-db-(cluster|instance)'
	'aws[[:space:]]+s3[[:space:]]+rb'
	'aws[[:space:]]+s3api[[:space:]]+delete-bucket'
	'aws[[:space:]]+kms[[:space:]]+schedule-key-deletion'
	'aws[[:space:]]+iam[[:space:]]+(delete|detach|put)-'
	'aws[[:space:]]+organizations[[:space:]]+'
	'aws[[:space:]]+ec2[[:space:]]+terminate-instances'
	'terraform([[:space:]]+-[^[:space:]]+)*[[:space:]]+(apply|destroy)'
	'kubectl[[:space:]]+delete'
)
for pattern in "${destructive_patterns[@]}"; do
	if [[ $bare =~ $pattern ]]; then
		decide deny "Destructive infrastructure command. All infrastructure here is Terraform under infra/ and is applied by a human, never from an agent session."
	fi
done

# --- reads pass silently ----------------------------------------------------
# Checked before the generic mutation sweep so a read whose SUBCOMMAND happens
# to contain a mutating word (describe-update, get-deletion-status) is not
# escalated.
if [[ $bare =~ (aws|kubectl)[[:space:]]+[a-z0-9-]+[[:space:]]+(describe|list|get|ls|head|wait|logs|version)([[:space:]-]|$) ]]; then
	exit 0
fi
if [[ $bare =~ aws[[:space:]]+(sts|sso)[[:space:]] ]]; then
	exit 0
fi
if [[ $bare =~ terraform([[:space:]]+-[^[:space:]]+)*[[:space:]]+(plan|validate|fmt|init|show|output|version) ]]; then
	exit 0
fi

# --- everything else that mutates: ask -------------------------------------
if [[ $bare =~ ^[[:space:]]*(aws|terraform|kubectl|helm)[[:space:]] ]] &&
	[[ $bare =~ (create|delete|put|update|modify|terminate|remove|[[:space:]]rm[[:space:]]|apply|destroy|attach|detach|scale|patch|set-|enable|disable|upgrade|install|uninstall) ]]; then
	decide ask "This mutates AWS or Kubernetes state from an agent session. Prefer Terraform under infra/ and a human apply."
fi

exit 0
