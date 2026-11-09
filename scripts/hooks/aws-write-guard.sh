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
#
# UNDER CODEX THERE IS NOBODY TO ASK, AND `ask` FAILS OPEN. Measured against
# Codex 0.167.0: a hook returning `permissionDecision: ask` is logged
# `PreToolUse Failed` and the command then RUNS. Only `deny` (or `exit 2`)
# blocks. So the middle case — every mutating aws/terraform/kubectl/helm call —
# would be the widest hole in the guard on exactly the agent that has no
# prompt to fall back on.
#
# `ER_GUARD_ASK_AS_DENY=1` collapses `ask` to `deny` for that caller.
# `scripts/codex-hooks-sync.sh` sets it; Claude Code does not, and keeps the
# prompt. The flag lives here rather than in a Codex-side wrapper so there is
# one decision table, and so this file's own tests cover both mappings.
#
# A COMMAND LINE IS NOT ONE COMMAND. The table below used to anchor on the
# first word of the whole string, and both failure directions were measured
# (#926): `cat <<YAML | kubectl apply -f -` and `make cluster-objects`
# sailed through unexamined, while a `gh pr create` whose heredoc BODY —
# data, not commands — contained the string `kubectl delete` was denied. So:
# heredoc bodies are stripped before anything matches, the rest is split
# everywhere a command can start (pipes, `&&`/`||`, `;`, newlines), and each
# segment is graded by its own head. Two documented approximations, both
# failing toward refusal: a multi-line QUOTED string (rather than a heredoc)
# still splits on its newlines, and a `bash -c '…'` wrapper shows only `bash`
# as the segment head — the destructive list scans the whole stripped string
# rather than one segment, so the common evasions stay denied, but the graded
# sweep does not see inside `-c`. The IAM denies are the layer that cannot be
# argued with; this is a seatbelt over them, with a test table in
# scripts/ci/codex-hooks-sync-test.sh.
set -uo pipefail

payload=$(cat)
command=$(printf '%s' "$payload" | jq -r '.tool_input.command // ""')

# Nothing to inspect — let the normal permission flow handle it.
[[ -n $command ]] || exit 0

decide() {
	# $1 = deny|ask, $2 = reason
	local d=$1
	# See the header: under Codex `ask` is not a weaker deny, it is an allow.
	if [[ $d == ask && ${ER_GUARD_ASK_AS_DENY:-0} == 1 ]]; then
		d=deny
		set -- "$d" "$2 (no interactive approval is available on this agent, so this is refused rather than escalated — run it yourself, or use a session that can prompt)"
	fi
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
	if ! [[ $command =~ [[:space:]](describe|list|get|ls|head|wait|logs|version)[a-z-]*([[:space:]]|$) ]]; then
		decide ask "er-builder is the never-expiring AdministratorAccess key, and no guardrail layer constrains it — not the MCP condition keys, which apply to permission sets rather than IAM user keys. Confirm this deliberately."
	fi
fi

# --- heredoc bodies are data, not commands ---------------------------------
# `gh pr create --body "$(cat <<EOF … kubectl delete … EOF)"` put the deny
# list's trigger string into a PR body, and the old substring match could not
# tell. Strip from each `<<MARKER` to the line that is only the marker. The
# MARKER LINE is kept: `cat <<YAML | kubectl apply -f -` carries a real
# command past the pipe on it.
hd_re='<<-?[[:space:]]*["'"'"']?([A-Za-z_][A-Za-z0-9_]*)["'"'"']?'
stripped=
terminator=
in_body=0
while IFS= read -r line; do
	if ((in_body)); then
		term_re="^[[:space:]]*${terminator}[[:space:]]*$"
		[[ $line =~ $term_re ]] && in_body=0
		continue
	fi
	stripped+="$line"$'\n'
	if [[ $line =~ $hd_re ]]; then
		terminator=${BASH_REMATCH[1]}
		in_body=1
	fi
done <<< "$command"

# --- unambiguously destructive ---------------------------------------------
# Deliberately narrow: each of these destroys state that is either production
# or expensive to rebuild, and none has a legitimate agent-initiated use.
# Scanned over the whole stripped string (not one segment) so a `bash -c`
# wrapper does not hide them.
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
	if [[ $stripped =~ $pattern ]]; then
		decide deny "Destructive infrastructure command — refused. The rule here: Terraform applies are CI-owned (merge to main is the only trigger), and Kubernetes cluster objects are applied by cluster-objects.yml on merge or by make cluster-objects with disclosure. Deletes and teardown are a human's to run."
	fi
done

# A narrowly parsed local manifest check creates no API object. Require both
# client dry-run and disabled API schema validation; reject unknown flags,
# duplicate flags and shell syntax instead of guessing what kubectl will see.
# This intentionally does not exempt exec, apply, delete or server dry-runs.
is_kubectl_client_check() {
	local text=$1 token value i=1 create=0 dry=0 validation=0 file=0 output=0
	local -a words
	[[ $text =~ ^[[:space:]A-Za-z0-9_./:=,@+-]+$ ]] || return 1
	read -r -a words <<< "$text"
	[[ ${words[0]:-} == kubectl ]] || return 1
	while ((i < ${#words[@]})); do
		token=${words[$i]}
		if ((create == 0)); then
			case $token in
			create) create=1 ;;
			--context | --namespace | --kubeconfig | -n)
				i=$((i + 1))
				value=${words[$i]:-}
				[[ -n $value && $value != -* ]] || return 1
				;;
			--context=?* | --namespace=?* | --kubeconfig=?*) ;;
			*) return 1 ;;
			esac
		else
			case $token in
			--dry-run=client) dry=$((dry + 1)) ;;
			--validate=false) validation=$((validation + 1)) ;;
			-f | --filename | -o | --output)
				i=$((i + 1))
				value=${words[$i]:-}
				[[ -n $value && $value != -* ]] || return 1
				case $token in
				-f | --filename) file=$((file + 1)) ;;
				*)
					[[ $value == name || $value == json || $value == yaml ]] || return 1
					output=$((output + 1))
					;;
				esac
				;;
			--filename=?*)
				value=${token#--filename=}
				[[ $value != -* ]] || return 1
				file=$((file + 1))
				;;
			--output=name | --output=json | --output=yaml) output=$((output + 1)) ;;
			*) return 1 ;;
			esac
		fi
		i=$((i + 1))
	done
	[[ $create == 1 && $dry == 1 && $validation == 1 && $file == 1 && $output -le 1 ]]
}

# --- grade every command on the line, not just the first --------------------
# Split everywhere a command can start. Each segment is env-stripped and
# graded by its own head, so `… | kubectl apply -f -` and
# `kubectl get pods && kubectl apply -f x` are both seen — the second of
# those passed silently before, because the reads exemption matched the
# whole string unanchored.
segments=${stripped//||/$'\n'}
segments=${segments//&&/$'\n'}
segments=${segments//|/$'\n'}
segments=${segments//;/$'\n'}

read_re='(aws|kubectl)[[:space:]]+[a-z0-9-]+[[:space:]]+(describe|list|get|ls|head|wait|logs|version)([[:space:]-]|$)'
sso_re='^[[:space:]]*aws[[:space:]]+(sts|sso)[[:space:]]'
tf_read_re='^[[:space:]]*terraform([[:space:]]+-[^[:space:]]+)*[[:space:]]+(plan|validate|fmt|init|show|output|version)'
head_re='^[[:space:]]*(aws|terraform|kubectl|helm)[[:space:]]'
env_re='^[[:space:]]*[A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+(.*)$'
mutating_re='(create|delete|put|update|modify|terminate|remove|[[:space:]]rm[[:space:]]|apply|destroy|attach|detach|scale|patch|set-|enable|disable|upgrade|install|uninstall)'
cluster_objects_re='(^|[[:space:]])cluster-objects([[:space:]]|$)'

pending_ask=
while IFS= read -r seg; do
	# Strip leading environment assignments so `AWS_PROFILE=x aws …` grades
	# as an `aws` segment — also the exact shape the er-builder key is used in.
	while [[ $seg =~ $env_re ]]; do
		seg=${BASH_REMATCH[1]}
	done
	[[ -n ${seg//[[:space:]]/} ]] || continue

	# The documented manual cluster-apply path. The guard cannot see inside
	# make, so the target name is the trigger.
	if [[ $seg =~ ^[[:space:]]*make[[:space:]] ]] && [[ $seg =~ $cluster_objects_re ]]; then
		pending_ask="make cluster-objects applies the infra/k8s objects to a cluster. On main that is CI's job (cluster-objects.yml); a manual run is the disclosed exception — confirm it is deliberate."
		continue
	fi

	if is_kubectl_client_check "$seg"; then
		continue
	fi

	# Reads pass, per segment. Checked before the mutation sweep so a read
	# whose SUBCOMMAND contains a mutating word (describe-update,
	# get-deletion-status) is not escalated.
	if [[ $seg =~ $read_re || $seg =~ $sso_re || $seg =~ $tf_read_re ]]; then
		continue
	fi

	# Everything else that mutates: ask.
	if [[ $seg =~ $head_re && $seg =~ $mutating_re ]]; then
		pending_ask="This mutates AWS or Kubernetes state from an agent session. Terraform applies are CI-owned (merge to main is the only trigger); cluster objects are applied by cluster-objects.yml on merge, or by make cluster-objects with disclosure."
	fi
done <<< "$segments"

[[ -n $pending_ask ]] && decide ask "$pending_ask"

exit 0
