#!/usr/bin/env bash
# Self-test for the Codex guard, Symposium startup, and their installer.
#
# Three things are asserted, and the order reflects which failure is worst:
#
#   1. THE GUARD STILL DENIES. A guard is decoration the moment its decision
#      table stops firing, and that failure is silent at runtime — Codex logs
#      `Completed` and runs the command. Every deny path is driven here with
#      Codex's own payload shape.
#   2. THE DERIVATION IS NOT EMPTY. The deny list is read from
#      `.claude/settings.json` rather than restated, so the way it breaks is a
#      renamed key producing zero patterns — which looks exactly like a clean
#      guard.
#   3. `ask` COLLAPSES TO `deny` ONLY FOR CODEX. Measured against Codex
#      0.167.0: a hook returning `ask` is logged `PreToolUse Failed` and the
#      command RUNS. Claude Code must keep the prompt, Codex must not get one,
#      and a change that unified them either way would be a regression in one
#      of the two agents.
#
# No `codex` on PATH is needed: the guard is a filter, so it is driven with
# recorded payloads. The end-to-end proof — a real session refusing a real
# command — is in #826 and the pull request body, because CI cannot run Codex.
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2
root="$(pwd)"
guard="$root/scripts/hooks/agent-pretooluse-guard.sh"
aws_guard="$root/scripts/hooks/aws-write-guard.sh"
settings="$root/.claude/settings.json"
sync="$root/scripts/codex-hooks-sync.sh"
symposium_start="$root/scripts/hooks/symposium-session-start.sh"

for f in "$guard" "$aws_guard" "$settings" "$sync" "$symposium_start"; do
	[ -f "$f" ] || {
		printf 'codex-hooks-sync-test: %s is missing (renamed?)\n' "$f" >&2
		exit 2
	}
done

command -v jq >/dev/null 2>&1 || {
	echo 'codex-hooks-sync-test: jq is required' >&2
	exit 2
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pass=0
fail=0

ok() {
	pass=$((pass + 1))
	printf '  PASS  %s\n' "$1"
}
no() {
	fail=$((fail + 1))
	printf '  FAIL  %s\n' "$1" >&2
	[ -n "${2:-}" ] && printf '        %s\n' "$2" >&2
	return 0
}

# decision <script> <command> [env...] -> prints deny|ask|allow|pass
decision() {
	local script=$1 cmd=$2
	shift 2
	local payload out
	payload=$(jq -nc --arg c "$cmd" '{tool_name:"Bash", tool_input:{command:$c}}')
	out=$(printf '%s' "$payload" | env "$@" bash "$script" 2>/dev/null)
	if [ -z "$out" ]; then
		printf 'pass\n'
		return
	fi
	printf '%s' "$out" | jq -r '.hookSpecificOutput.permissionDecision // "unparseable"'
}

# --- 1. the guard still denies ---------------------------------------------
# Each command is assembled from fragments so this file never contains a
# runnable destructive command: a test fixture that is also a live command is
# one careless copy-paste from being run.
expect_deny() {
	local label=$1 cmd=$2 got
	got=$(decision "$guard" "$cmd")
	[ "$got" = deny ] && ok "denies: $label" || no "denies: $label" "got '$got' for: $cmd"
}
expect_pass() {
	local label=$1 cmd=$2 got
	got=$(decision "$guard" "$cmd")
	[ "$got" = pass ] && ok "passes: $label" || no "passes: $label" "got '$got' for: $cmd"
}

expect_deny 'pkill, from the settings deny list' "pkill -f 'make check'"
expect_deny 'pkill with no arguments' "pkill"
expect_deny 'killall' "killall node"
expect_deny 'a deny-list entry behind an env prefix' "AWS_PROFILE=x aws organizations""  list-accounts"
expect_deny 'bucket removal' "aws s3 ""rb s3://some-bucket"
expect_deny 'cluster deletion' "aws eks ""delete-cluster --name er-dev"
expect_deny 'kubectl deletion' "kubectl ""delete pod x"
expect_deny 'a terraform apply' "terraform ""apply -auto-approve"
expect_deny 'the mutating sweep (was ask)' "kubectl ""patch pod x -p '{}'"

# #926: the two invocation shapes that bypassed the first-word anchor, the
# multi-line shape whose read prefix used to exempt a later mutation, and the
# heredoc false positive that came of matching substrings instead of commands.
expect_deny 'a guarded call past a pipe' "echo yaml | kubectl ""apply -f -"
expect_deny 'a heredoc piped into kubectl' "cat <<YAML | kubectl ""apply -f -"
expect_deny 'make cluster-objects' "make cluster-objects ""ER_ENV=prod"
expect_deny 'a mutation on the second line' "$(printf 'kubectl get pods\nkubectl %s-f x.yaml' 'apply')"

expect_pass 'an aws read' 'aws eks describe-cluster --name er-dev'
expect_pass 'a terraform plan' 'terraform plan'
expect_pass 'an sts call' 'aws sts get-caller-identity'
expect_pass 'an unrelated command' 'echo hello'
expect_pass 'a tool call with no command at all' ''
expect_pass 'a read past a pipe' 'kubectl get pods | grep Running'

# A local client-side manifest check must not be mistaken for a create.
expect_pass 'local kubectl client dry run' 'kubectl create --dry-run=client --validate=false -f manifest.json -o name'
expect_pass 'client dry run with global options and reordered flags' 'kubectl --context er-prod -n app create -o json -f ./manifest.yaml --validate=false --dry-run=client'
expect_pass 'client dry run with equals-form file and output' 'kubectl create --filename=./manifest.yaml --output=yaml --dry-run=client --validate=false'
expect_deny 'real kubectl create remains guarded' 'kubectl create --validate=false -f manifest.json -o name'
expect_deny 'server dry run remains guarded' 'kubectl create --dry-run=server --validate=false -f manifest.json'
expect_deny 'conflicting dry-run flags are not exempt' 'kubectl create --dry-run=client --dry-run=none --validate=false -f manifest.json'
expect_deny 'duplicate dry-run flags are not exempt' 'kubectl create --dry-run=client --dry-run=client --validate=false -f manifest.json'
expect_deny 'dry-run text as a filename is not a flag' 'kubectl create -f --dry-run=client --validate=false'
expect_deny 'a local check does not exempt a following write' 'kubectl create --dry-run=client --validate=false -f x.json; kubectl create -f y.json'
expect_deny 'client check with an unknown option is not exempt' 'kubectl create --dry-run=client --validate=false --raw=/api/v1/namespaces -f x.json'
expect_deny 'quoted dry-run data is not an option' "kubectl create configmap x --from-literal='example=--dry-run=client' --validate=false"
expect_deny 'substitutions are not exempt' 'kubectl create --dry-run=client --validate=false -f $(echo manifest.json)'


# The measured false positive: a PR body is data. Assembled so this file never
# holds a runnable destructive command, same as the deny cases above.
heredoc_with_delete=$(printf 'gh pr create --body "$(cat <<'"'"'EOF'"'"'\n%s\nEOF\n)"' "kubectl ""delete pod x")
expect_pass 'a deny-list string inside a heredoc body' "$heredoc_with_delete"

# --- 2. the derivation is not empty ----------------------------------------
n=$(jq -r '[.permissions.deny[]? | select(startswith("Bash("))] | length' "$settings")
if [ "${n:-0}" -gt 0 ]; then
	ok "the deny list derives $n Bash(...) pattern(s) from settings.json"
else
	no "the deny list derived nothing from $settings" \
		"a renamed key here disables half the guard silently"
fi

# And the installer refuses to install against an empty one, rather than
# installing a guard that protects less than it says.
mkdir -p "$work/fakerepo/scripts/hooks" "$work/fakerepo/.claude"
cp "$guard" "$aws_guard" "$symposium_start" "$work/fakerepo/scripts/hooks/"
printf '{"permissions":{"deny":[]}}\n' >"$work/fakerepo/.claude/settings.json"
if out=$(bash "$sync" --dry-run --root "$work/fakerepo" --codex-home "$work/home" 2>&1); then
	no "the installer accepted an empty deny list" "$out"
else
	case "$out" in
	*"which is not a guard"*) ok "the installer refuses an empty deny list" ;;
	*) no "the installer refused an empty deny list for the wrong reason" "$out" ;;
	esac
fi

# --- 3. ask collapses to deny only for Codex -------------------------------
# Driven against aws-write-guard directly, because the mapping lives there.
mid="kubectl ""patch pod x -p '{}'"
claude_side=$(decision "$aws_guard" "$mid")
codex_side=$(decision "$aws_guard" "$mid" ER_GUARD_ASK_AS_DENY=1)
[ "$claude_side" = ask ] &&
	ok "Claude Code still gets 'ask' for the mutating sweep" ||
	no "Claude Code no longer gets 'ask'" "got '$claude_side' — the prompt is the reason that case is not a deny"
[ "$codex_side" = deny ] &&
	ok "Codex gets 'deny' for the same command" ||
	no "Codex does not get 'deny'" "got '$codex_side' — on Codex 'ask' is logged Failed and the command RUNS"

# The #926 graded shapes keep the same mapping: ask on Claude Code, deny on
# Codex. Driving aws-write-guard directly, because the mapping lives there.
for graded in "make cluster-objects ""ER_ENV=prod" "echo yaml | kubectl ""apply -f -"; do
	[ "$(decision "$aws_guard" "$graded")" = ask ] &&
		ok "Claude Code gets 'ask' for a graded shape" ||
		no "Claude Code no longer gets 'ask' for a graded shape" "got '$(decision "$aws_guard" "$graded")' for: $graded"
	[ "$(decision "$aws_guard" "$graded" ER_GUARD_ASK_AS_DENY=1)" = deny ] &&
		ok "Codex gets 'deny' for the same shape" ||
		no "Codex does not get 'deny' for a graded shape" "got '$(decision "$aws_guard" "$graded" ER_GUARD_ASK_AS_DENY=1)' for: $graded"
done

# A hard deny must not be softened by the flag in either direction.
hard="aws s3 ""rb s3://some-bucket"
[ "$(decision "$aws_guard" "$hard")" = deny ] && [ "$(decision "$aws_guard" "$hard" ER_GUARD_ASK_AS_DENY=1)" = deny ] &&
	ok "a hard deny is identical in both modes" ||
	no "a hard deny differs between modes"

# --- Symposium startup derives from Claude's hook, not a second command ----
mkdir -p "$work/fakerepo/.tools/bin" "$work/fakerepo/rust" "$work/fakerepo/.symposium"
cp "$settings" "$work/fakerepo/.claude/settings.json"
cat >"$work/fakerepo/.tools/bin/cargo-agents" <<'EOF'
#!/usr/bin/env bash
payload=$(cat)
jq -nc --arg home "$SYMPOSIUM_HOME" --arg args "$*" --arg pwd "$PWD" --argjson payload "$payload" \
	'{home:$home,args:$args,pwd:$pwd,payload:$payload}'
EOF
chmod +x "$work/fakerepo/.tools/bin/cargo-agents"
bash "$sync" --root "$work/fakerepo" --codex-home "$work/fakehome" >/dev/null 2>&1
startup=$(jq -r '.hooks.SessionStart[]?.hooks[]? | select(.description=="er-codex-symposium-session-start") | .command' "$work/fakehome/hooks.json")
payload=$(jq -nc --arg cwd "$work/fakerepo" '{hook_event_name:"SessionStart",session_id:"root-session",cwd:$cwd,source:"startup"}')
got=$(printf '%s\n' "$payload" | bash -c "$startup")
if printf '%s\n' "$got" | jq -e --arg root "$work/fakerepo" \
	'.home == ($root + "/.symposium") and .args == "hook codex session-start" and .pwd == ($root + "/rust") and .payload.cwd == ($root + "/rust") and .payload.session_id == "root-session" and .payload.source == "startup"' >/dev/null; then
	ok "Codex startup rewrites a repo-root payload to the Rust workspace"
else
	no "Codex startup did not use the Rust workspace" "got '$got'"
fi

claude_startup=$(jq -r '[.hooks.SessionStart[]?.hooks[]?.command // empty | select(contains("scripts/hooks/symposium-session-start.sh"))] | .[0]' "$work/fakerepo/.claude/settings.json")
got=$(printf '%s\n' "$payload" | CLAUDE_PROJECT_DIR="$work/fakerepo" bash -c "$claude_startup")
if printf '%s\n' "$got" | jq -e --arg root "$work/fakerepo" \
	'.home == ($root + "/.symposium") and .args == "hook claude session-start" and .pwd == ($root + "/rust") and .payload.cwd == ($root + "/rust") and .payload.session_id == "root-session" and .payload.source == "startup"' >/dev/null; then
	ok "Claude startup rewrites a repo-root payload to the Rust workspace"
else
	no "Claude startup did not use the Rust workspace" "got '$got'"
fi

jq 'del(.hooks.SessionStart)' "$settings" >"$work/fakerepo/.claude/settings.json"
if out=$(bash "$sync" --dry-run --root "$work/fakerepo" --codex-home "$work/fakehome" 2>&1); then
	no "the installer accepted a missing Claude Symposium startup" "$out"
else
	case "$out" in
	*"expected one Symposium SessionStart"*) ok "a missing startup hook fails instead of silently dropping Codex parity" ;;
	*) no "missing startup hook failed for the wrong reason" "$out" ;;
	esac
fi

# --- the installer is idempotent and leaves other hooks alone --------------
home="$work/home2"
mkdir -p "$home"
printf '{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"true","description":"not-ours"}]}]}}\n' >"$home/hooks.json"
bash "$sync" --root "$root" --codex-home "$home" >/dev/null 2>&1
bash "$sync" --root "$root" --codex-home "$home" >/dev/null 2>&1
pre=$(jq -r '[.hooks.PreToolUse[]?.hooks[]? | select(.description=="er-codex-pretooluse-guard")] | length' "$home/hooks.json")
startup_count=$(jq -r '[.hooks.SessionStart[]?.hooks[]? | select(.description=="er-codex-symposium-session-start")] | length' "$home/hooks.json")
other=$(jq -r '[.hooks.SessionStart[]?.hooks[]? | select(.description=="not-ours")] | length' "$home/hooks.json")
{ [ "$pre" = 1 ] && [ "$startup_count" = 1 ] && [ "$other" = 1 ]; } &&
	ok "installing twice leaves one of each derived hook and preserves another hook" ||
	no "the installer is not idempotent or clobbered another hook" "guard=$pre startup=$startup_count other=$other"

bash "$sync" --remove --root "$root" --codex-home "$home" >/dev/null 2>&1
pre=$(jq -r '[.hooks.PreToolUse[]?.hooks[]? | select(.description=="er-codex-pretooluse-guard")] | length' "$home/hooks.json")
startup_count=$(jq -r '[.hooks.SessionStart[]?.hooks[]? | select(.description=="er-codex-symposium-session-start")] | length' "$home/hooks.json")
other=$(jq -r '[.hooks.SessionStart[]?.hooks[]? | select(.description=="not-ours")] | length' "$home/hooks.json")
{ [ "$pre" = 0 ] && [ "$startup_count" = 0 ] && [ "$other" = 1 ]; } &&
	ok "--remove takes both derived hooks out and leaves the rest" ||
	no "--remove removed the wrong thing" "guard=$pre startup=$startup_count other=$other"

echo "---"
printf 'codex-hooks-sync-test: %d passed, %d failed\n' "$pass" "$fail"
# A run that asserted nothing is a failure, same contract as every other
# checker in scripts/ci.
if [ "$pass" -eq 0 ]; then
	echo 'FAIL asserted nothing' >&2
	exit 2
fi
[ "$fail" -eq 0 ] || exit 1
