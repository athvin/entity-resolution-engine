# The three guardrail layers, and what each one misses

## Contents

- Why three layers
- Layer 1: the MCP proxy, client-side
- Layer 2: IAM condition keys, server-side
- Layer 3: the Bash guard
- The profile none of them constrains
- Verifying a layer by breaking it

## Why three layers

Each layer is individually bypassable, and the bypasses are not the same shape.
Stating them plainly, because the temptation is to configure one and feel
finished:

| Layer | Stops | Misses |
|---|---|---|
| MCP proxy `--read-only` | write tools being offered to the agent at all | any other MCP client, and everything outside MCP |
| IAM condition keys | MCP-routed mutation, at the API | **every CLI and SDK call** — the keys only exist on MCP-routed requests |
| Bash guard | `aws` / `terraform` / `kubectl` mutation from a session | non-shell paths, and anything its patterns do not match |

Layer 2 is the only one that is genuinely server-side, and it is also the one
with the largest blind spot. That asymmetry is the whole reason for three.

## Layer 1: the MCP proxy, client-side

`.mcp.json` at the repo root declares the AWS MCP Server behind
`mcp-proxy-for-aws-cli`, with `--read-only` so write-capable tools are never
offered, and a profile whose credentials the proxy signs with.

**SigV4, not OAuth.** Three capabilities require it and are the reason for the
local proxy at all: read-only mode, multi-profile switching, and setting a
default region for the session. OAuth binds a session to a single role and
cannot do any of the three.

Two configuration details that bite:

- **The endpoint region and the operating region are different things.** The
  region in the endpoint URL selects *which MCP server you reach*; the
  `AWS_REGION` metadata sets *the default region for operations*. They differ
  here — the resources are in `us-east-2`.
- **`AWS_MCP_PROXY_PROFILES` takes precedence** over both `--profile` and
  `AWS_PROFILE`. That is the documented escape hatch for plugin-bundled servers
  whose CLI args cannot be edited, and it means an environment variable can
  silently override the checked-in profile choice.

The proxy reads credentials from disk on every request, so refreshing an expired
SSO session takes effect without restarting anything. An `ExpiredTokenException`
means `aws sso login`, not a broken config.

### The profile this runs as

`er-readonly` → the `PlatformReadOnly` permission set, which carries the
AWS-managed `ReadOnlyAccess` policy with an 8-hour session.

That matters because it means the proxy's `--read-only` flag is **not** the only
thing standing between an agent and a mutation: the credential itself cannot
write. `ProdDataReadOnly` could not serve here — it covers the prod lake,
`rds-db:connect`, `eks:Describe*` and logs, so it cannot list buckets or read
Budgets, which is most of what introspection asks.

An expired SSO token is the usual reason the server answers nothing. The fix is
`aws sso login --profile er-readonly`, not a config change — the proxy re-reads
credentials from disk per request, so no restart is needed.

## Layer 2: IAM condition keys, server-side

Two context keys exist on requests that arrive through an AWS-managed MCP
server:

| Key | Type | Carries |
|---|---|---|
| `aws:ViaAWSMCPService` | Bool | true when the request came through an AWS-managed MCP server |
| `aws:CalledViaAWSMCP` | String | the server's service principal, e.g. `aws-mcp.amazonaws.com` |

The deny lives as the `DenyMutationViaMCP` statement in
`aws_ssoadmin_permission_set_inline_policy.platform_admin_guard` — the same
inline policy that already carries the prod-mutation denies, so this extends a
proven pattern rather than adding a resource. **Applied**, and verifiable
without Terraform:

```sh
aws sso-admin get-inline-policy-for-permission-set \
  --instance-arn "$INSTANCE" --permission-set-arn "$PLATFORM_ADMIN" \
  --query InlinePolicy --output text | jq '.Statement[] | select(.Sid=="DenyMutationViaMCP")'
```

**The critical limitation, restated because it is easy to forget:** these keys
apply *exclusively* to requests flowing through an AWS-managed MCP server.
A direct API call, an `aws` CLI invocation, or an SDK call from a workload
bypasses them entirely. Layer 2 does not make the account read-only; it makes
*MCP-routed* access read-only.

Two things to weigh when editing it:

- `ProdAdmin` carries `AdministratorAccess` with **no inline guard**, so it is
  unaffected by this statement. Whether it should get the same one is an open
  decision, not an oversight to fix silently.
- The other denies in that policy are conditioned on
  `aws:ResourceTag/Environment = prod`. An MCP deny conditioned only on
  `aws:ViaAWSMCPService` is therefore **broader** than its neighbours — it
  covers dev too. That is intended here, but it means the statement reads
  differently from the ones around it.

## Layer 3: the Bash guard

A `PreToolUse` hook gated by `if: "Bash(aws *)"`, `"Bash(terraform *)"` and
`"Bash(kubectl *)"`, plus a committed `permissions.deny` list for the
unambiguously destructive shapes.

Its decision rule, in order:

1. **Reads pass silently.** `describe`, `list`, `get`, `plan`, `validate`.
2. **Unambiguously destructive calls are denied** outright.
3. **Everything else mutating escalates to `ask`**, not `deny`, so a human
   decides and the workflow survives.

Three legitimate write paths already in the tree must keep working, and are
named exceptions rather than collateral:

- `scripts/dev_env.sh`'s `secretsmanager create-secret` / `delete-secret`
- `scripts/dev_env.sh`'s `s3 rm --recursive` against a dev lake prefix
- the Makefile's `aws ecr get-login-password`

Note on mechanism: `deny` and `ask` rules take effect immediately, while
`allow` rules wait for workspace trust. So the guard's restrictive half is
live on first clone and its permissive half is not — which is the safe
direction for this to fail in.

## The profile none of them constrains

`er-builder` is backed by a never-expiring `AdministratorAccess` static access
key created for the build loop. Against the three layers:

- Layer 1 does not use it.
- Layer 2 does not reach it — the deny lives on a *permission set's* inline
  policy, and this is an IAM user's key, with no permission set involved.
- Layer 3's patterns match the command, not the identity.

So the guard escalates to `ask` on **any** mutating command when
`AWS_PROFILE=er-builder` appears in the environment or on the command line,
regardless of verb. It is the one case where the identity, not the action, is
the trigger.

The real fix is deleting the key, which is already a documented commitment
before the production gate. Until then, treat its presence as the account's
widest hole.

## Verifying a layer by breaking it

A guardrail that has never been observed failing is a guess. Check each:

```
- [ ] aws eks delete-cluster er-dev           → denied by the Bash guard
- [ ] aws s3 ls                               → passes silently
- [ ] AWS_PROFILE=er-builder aws s3 cp …      → escalates to ask
- [ ] scripts/dev_env.sh secret path          → still works
- [ ] make image-api (ecr get-login-password) → still works
- [ ] a write question to the MCP server      → the tool is not offered at all
- [ ] /mcp                                    → exactly one AWS server connected
```

The last line matters more than it looks. AWS's own guidance warns that running
two AWS MCP servers at once produces duplicate tools that degrade agent
performance, and a plugin can bring its own server whose args cannot be edited —
so a second one appearing silently defeats Layer 1 without any error.
