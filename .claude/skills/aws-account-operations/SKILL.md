---
name: aws-account-operations
description: Explains how to operate and introspect this repo's live AWS account — the single-account model where environments are separated by name prefix and permission set rather than by account, so every guardrail is an IAM-layer deny and SCPs apply to nothing; the Identity Center permission sets, which CLI profile reaches each and which carries no guard; the Pod Identity rule binding one namespace and service account to one role; what is and is not alarmed, which is almost nothing; the Budgets and CUR 2.0 cost pipeline; and the two traps that cost real time, namely cost-allocation tags that never backfill and the 8-vCPU quotas that leave a runner pod pending forever. Use when asking what is deployed, investigating a bill, a quota error or a pod that cannot reach AWS, reading logs or CloudTrail, or reaching for the `aws` CLI. All infrastructure here is Terraform — never generate CDK, CloudFormation or SAM. For authoring Terraform and Helm see infra-platform; for CI and image builds see ci-and-supply-chain.
---

# Operating the account

This skill is about the account as it is running. Authoring the Terraform that
produces it belongs to the infra-platform skill, which also owns the module
layout, EKS Auto Mode and the Helm charts.

> **There is one AWS account.** Environments are separated by name prefix
> (`er-dev-*` / `er-prod-*`) and by permission set — not by account. The
> organization exists only because Identity Center requires an org instance.

Everything awkward about operating this account follows from that sentence.

## All infrastructure is Terraform. Never CDK, CloudFormation or SAM.

Terraform under `infra/terraform/` is the only infrastructure language in this
repo. There is no `cdk.json`, no `template.yaml`, no `samconfig`, and no
CloudFormation anywhere — and that is a decision, not a gap.

This matters because general-purpose AWS skills and the AWS MCP server's own
guidance default to CDK or CloudFormation. **When a task needs a new resource,
the answer is a Terraform resource in the right stack under
`infra/terraform/envs/` or a module under `infra/terraform/modules/`.** A CDK
app or a CFN template is not a smaller first step here; it is a second,
unmanaged source of truth for the same account.

The one legitimate use of CloudFormation knowledge is *reading* — an EKS addon
or a service-linked resource AWS manages on your behalf may surface as a stack.
Read it; do not add to it.

## Every guardrail is an IAM deny, one edit from absent

Because there is a single account, a Service Control Policy has nothing to
apply to. The `SERVICE_CONTROL_POLICY` type stays enabled with no custom SCP,
and prod is protected instead by **Deny statements on the permission set a
developer actually assumes**.

The consequences, which `docs/infrastructure.md` §3 records as deliberate
trade-aways rather than oversights:

- A guardrail is one policy edit from gone, and nothing above the account can
  restore it.
- Blast radius and **service quotas are shared** between dev and prod. A dev
  experiment can exhaust a quota prod needs.
- The audit archive lives beside the workloads it records, compensated by S3
  Object Lock in compliance mode rather than by account separation.

So when reasoning about whether something is safe, the question is never "which
account is this in" — it is "which permission set is assumed, and what does its
inline policy deny".

## Permission sets, and the profiles that reach them

Access is by **group membership**, not per-person assignment. Human IAM users do
not exist; see the reference for the one long-lived key that does.

| Permission set | Session | Reached by |
|---|---|---|
| `PlatformAdmin` | PT8H | the `er-platform` CLI profile |
| `PlatformReadOnly` | PT8H | **not applied yet** — `er-readonly`, once it is |
| `ProdDataReadOnly` | PT4H | no profile configured |
| `ProdAdmin` | PT1H | the `er-prodadmin` CLI profile |

Three things to know before picking one. `PlatformAdmin` carries
`AdministratorAccess` *plus* an inline guard whose Deny statements block prod
S3, RDS, EKS, Secrets and KMS mutation (conditioned on
`aws:ResourceTag/Environment = prod`), prod role minting, and
`iam:CreateAccessKey`. `ProdAdmin` carries `AdministratorAccess` with **no
inline guard at all** — it is the break-glass identity, and its one-hour session
is the only thing limiting it.

`ProdDataReadOnly` looks like the obvious read-only identity but is much
narrower than its name suggests: lake reads, `rds-db:connect` as `er_readonly`,
`eks:Describe*` and log reads. It cannot list buckets or read Budgets, so it is
not a general introspection identity. `PlatformReadOnly` is the one that is —
account-wide `ReadOnlyAccess`, added for the MCP proxy — but it is **written
and validated, not applied**, so until a human applies it the proxy runs as
`PlatformAdmin`.

The generated reference carries an **Inline policy** column, and a `**none**`
there is the fact worth noticing: a permission set with no inline policy has no
deny statements at all, whatever its managed policy allows.

## Introspecting the account

A read-only question about the live account should go through the **AWS MCP
Server**, declared in `.mcp.json` behind `mcp-proxy-for-aws-cli` with
`--read-only`. It beats a shell `aws` call on two counts: write-capable tools
are never offered to the agent at all, and every request is recorded in
CloudWatch and CloudTrail as agent-initiated, which a Bash invocation is not.

Its tools are fully qualified — `aws___search_documentation` for current AWS
behaviour (prefer it over recalling API details) and `aws___retrieve_skill` for
AWS's own service guidance on demand. Check `references/footprint.md` first
regardless: it is generated from the Terraform, so it answers anything
*declared* without touching the account.

**Three guardrail layers, none sufficient alone**, and they are not all applied
yet: the proxy flag is client-side and another client ignores it; the IAM
condition keys reach only calls made *through* the MCP server and so miss the
CLI entirely; the Bash guard catches the CLI but is pattern-based. Which are
live, which await a human `terraform apply`, and the one profile none of them
constrains:
[references/mcp-and-guardrails.md](references/mcp-and-guardrails.md).

The operative rule stands regardless of which layers are applied: **do not
mutate AWS from an agent session.** Write Terraform and let a human apply it.

## Pod Identity, never IRSA — and one role per triple

A workload gets AWS access through `aws_eks_pod_identity_association`, trusting
`pods.eks.amazonaws.com`. There is no OIDC provider for workloads; the only one
in the account is for GitHub Actions pushing to ECR.

The rule with a real design consequence: **an association binds one (cluster,
namespace, service account) triple to one role, and there is no wildcard.** So
a pod needing two services gets **two policies on one role**, not two roles —
which is exactly why the SES and Bedrock grants both hang off the single
`er-dev-api-pod` role.

A pod that cannot reach AWS is almost always a missing association rather than a
missing policy. Note also that the IMDS hop limit makes node-role fallback
unreachable, so there is no silent degradation to a broader credential — the
call just fails, which is the desired behaviour.

## What is not alarmed

Worth knowing before assuming a problem would have paged someone:

- Two CloudWatch log groups exist, both with finite retention. **Zero
  dashboards, zero metric alarms, zero metric filters, zero SNS topics.**
- Budget notifications go to **one email address** and nowhere else. That is the
  entire alerting surface.
- Fluent Bit is deliberately **not** granted `logs:CreateLogGroup`, so a shipper
  cannot invent a retention-less group. The OTel collector is granted it, scoped
  to one group ARN.

So "nothing alerted, so nothing is wrong" is not sound reasoning here. Equally,
proposing a dashboard or alarm is adding the first one, not extending a set.

## Two traps that cost real time

**Cost-allocation tags do not backfill.** Resources carry `Project`,
`Environment`, `ManagedBy` and sometimes `Developer` through the provider's
`default_tags`, but a tag is useless for cost until it is **activated in the
Billing console** — activation takes up to 24 hours and does not apply
retroactively. Spend before activation is permanently unattributable.

**A fresh account's vCPU quotas silently prevent large nodes from placing.**
*All Standard Spot Instance Requests* (`L-34B43A08`) and *Running On-Demand
Standard instances* (`L-1216C47A`) default to **8 vCPU each** — measured in
vCPUs, not instances. A 16-vCPU runner node therefore can never place: the
fleet request fails `VcpuLimitExceeded` / `MaxSpotInstanceCountExceeded`, the
NodeClaim is torn down and retried, and the pod sits `Pending` forever while the
dispatcher keeps the job active. Both are raised to 64 in `us-east-2`. A pod
stuck `Pending` with no scheduling event is this, until proven otherwise.

Budgets, the CUR 2.0 pipeline and per-tenant attribution:
[references/cost-and-billing.md](references/cost-and-billing.md).

What the account actually holds — resource types and counts, permission sets,
the AWS SDK surface in the Rust workspace:
[references/footprint.md](references/footprint.md).

Bedrock's two halves, the SDK feature pins that must not be relaxed, and why
`aws sso login` does not work for a locally-run `er-server`:
[references/bedrock-and-sdks.md](references/bedrock-and-sdks.md).

## Common workflows

**"What is deployed / what version / what is this resource?"** Check
`references/footprint.md` first — generated from the Terraform, so it answers
anything declared without touching the account. For live state, ask the MCP
server.

**A pod cannot reach AWS.** Check for an `aws_eks_pod_identity_association`
covering its exact (cluster, namespace, service account) triple before looking
at the policy. Adding a second role for a pod that already has one is the
common wrong turn.

**A pod is stuck `Pending`.** Check the vCPU quotas above before anything else.

**"Why did the bill move?"** Budgets only tell you *that* it did. The CUR 2.0
export with split cost allocation is the only thing that answers *why* — see
the cost reference. If the period predates tag activation, the answer may not
exist.

**A new AWS resource is needed.** Write Terraform. Then `terraform fmt
-check -recursive` and a credential-free `init -backend=false && validate`,
which is what CI runs. Do not apply from an agent session.

**Reaching for the `aws` CLI to change something.** Prefer Terraform. If a
one-off genuinely is right, expect the Bash guard to deny it outright or ask
first — that is the guard working, not a bug.
