---
name: aws-account-operations
description: Explains how to operate and introspect this repo's live AWS account — the single-account model where environments are separated by name prefix and permission set, so every guardrail is an IAM deny; the Identity Center permission sets and which CLI profile reaches each; the Pod Identity one-role-per-triple rule; what is alarmed; the Budgets and CUR 2.0 pipeline; and two traps — cost-allocation tags never backfill, vCPU quotas leave a pod Pending. Use when asking what is deployed, investigating a bill, a quota error or a pod that cannot reach AWS, reading logs or CloudTrail, or reaching for the `aws` CLI. All infrastructure is Terraform — never CDK, CloudFormation or SAM. For authoring Terraform and Helm see infra-platform; for CI and image builds see ci-and-supply-chain; for the AWS feature pins see rust-conventions; for lake credentials see ducklake-lakehouse; for emitting telemetry see observability; for the health sweep that files board cards see health-sweep.
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

The consequences, which `.claude/skills/infra-platform/references/infrastructure.md` §3 records as deliberate
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

The sets, their session lengths and **which carry an inline policy** are the
generated table in [references/footprint.md](references/footprint.md). The CLI
profile that reaches each: `er-platform` → `PlatformAdmin`, `er-readonly` →
`PlatformReadOnly`, `er-prodadmin` → `ProdAdmin`. `ProdDataReadOnly` has no
profile configured.

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
account-wide `ReadOnlyAccess`, added for the MCP proxy, which runs as it via the
`er-readonly` profile.

A `**none**` in that table's **Inline policy** column is the fact worth
noticing: a permission set with no inline policy has no deny statements at all,
whatever its managed policy allows.

## Introspecting the account

Three sources, and picking the wrong one wastes time:

| Question | Answer with |
|---|---|
| anything **declared** — what resources exist, which permission sets, the SDK surface | [references/footprint.md](references/footprint.md), generated from the Terraform |
| **live account state** — a cluster's version, a bucket's contents, a budget's spend | the `aws` CLI under the `er-readonly` profile |
| **how AWS itself behaves** — an API's shape, a service's current guidance | the AWS MCP Server |

**The MCP server does not reach account state, and that is by configuration.**
`.mcp.json` runs the proxy with `--read-only`, which strips `aws___run_script`
and `aws___get_presigned_url` — `run_script` being the only tool that executes
anything against the account. What remains is documentation, guidance and region
metadata; run `/mcp` to see the tools actually offered rather than trusting a
list here, since they track the vendor's server version. Prefer them over
recalling API details from memory; do not expect them to tell you what is
running.

So a live question is a CLI question:

```sh
aws sso login --profile er-readonly        # PlatformReadOnly, 8h
AWS_PROFILE=er-readonly aws eks describe-cluster --name er-dev --query cluster.version
```

That profile cannot write — `ReadOnlyAccess`, verified by
`iam simulate-principal-policy` returning `implicitDeny` for `s3:PutObject`,
`eks:DeleteCluster`, `iam:CreateUser` and the rest.

**An expired SSO token is the usual reason either the CLI or the MCP server
stops answering**, and the fix is another `aws sso login --profile er-readonly`
rather than a config change. The proxy re-reads credentials from disk per
request, so nothing needs restarting. (This is the one place that fact is
stated; it was previously repeated four times across this skill.)

**Three guardrail layers, none sufficient alone**: the proxy flag is
client-side and another client ignores it; the IAM condition keys reach only
calls made *through* the MCP server, so they miss the CLI entirely; the Bash
guard catches the CLI but is pattern-based. One consequence worth stating
plainly — because `--read-only` already removes `run_script`, the IAM layer
currently defends a path the config does not expose. It is there for the day
someone drops that flag, not as today's active control. What each layer misses,
and the one profile none of them constrains:
[references/mcp-and-guardrails.md](references/mcp-and-guardrails.md).

The operative rule is unchanged: **do not mutate AWS from an agent session.**
Write Terraform and let a human apply it.

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

## What is and is not alarmed

**Read the counts off the generated alerting-surface table in
[references/footprint.md](references/footprint.md), not off prose here.** This
section asserted "zero metric alarms, zero metric filters, zero SNS topics"
after two of those three had stopped being true, and it survived review because
the other two still were. The table is derived from `infra/terraform/`, so it
cannot drift that way again.

What the shape of that table means, which is the part worth carrying:

- **There are two notification paths, not one, and they are deliberately not
  merged.** Metric alarms publish to an SNS topic with an email subscription;
  the Budgets email the same address directly. `alerts.tf` explains why they stay
  separate — Budgets is served from `us-east-1` while a CloudWatch alarm can only
  publish to a topic in its own region, so "one path" would really be two topics
  sharing a subscriber, for no gain at one subscriber. **Adding a second
  destination is what justifies merging them**, and then the move is a `us-east-1`
  topic beside the existing one with both Budgets repointed.
- Dashboards and metric filters remain at zero. The metric filter is the sharper
  absence: nothing turns a log line into a metric, so an alarm can only watch
  what a service publishes natively.
- An SNS email subscription is `PENDING` until the address confirms it, Terraform
  cannot confirm it, and `alerts.tf` notes an unconfirmed subscription looks
  identical to a working one in the console. **Whether an alarm reaches anyone is
  a live check, not a read of the declaration** — `sns list-subscriptions-by-topic`
  and look for a `PendingConfirmation` ARN.
- Fluent Bit is deliberately **not** granted `logs:CreateLogGroup`, so a shipper
  cannot invent a retention-less group. The OTel collector is granted it, scoped
  to one group ARN.

So "nothing alerted, so nothing is wrong" is still not sound reasoning — but now
because the coverage is thin and one subscription may be unconfirmed, not because
there is nothing there at all.

## Two traps that cost real time

**Cost-allocation tags do not backfill.** Resources carry `Project`,
`Environment`, `ManagedBy` and sometimes `Developer` through the provider's
`default_tags`, but a tag is useless for cost until it is **activated in the
Billing console** — activation takes up to 24 hours and does not apply
retroactively. Spend before activation is permanently unattributable.

**The account's vCPU quotas silently prevent large nodes from placing, and
they are still binding.** *All Standard Spot Instance Requests*
(`L-34B43A08`) and *Running On-Demand Standard instances* (`L-1216C47A`) are
measured in vCPUs, not instances. When the ceiling is low a large node can
never place: the fleet request fails `VcpuLimitExceeded` /
`MaxSpotInstanceCountExceeded`, the NodeClaim is torn down and retried, and the
pod sits `Pending` with nothing obvious in its own events. **A pod stuck
`Pending` with no scheduling event is this, until proven otherwise.**

**"Until proven otherwise" has exactly one other candidate, and it is not in
this account.** A namespace `ResourceQuota` produces the same `Pending` symptom
from a different layer, and the two are told apart by what the platform tried:

| | Account vCPU ceiling | Namespace `ResourceQuota` |
|---|---|---|
| **Is there a pod?** | yes — it exists and is `Unschedulable`, while NodeClaims appear and are torn down | **no** — the quota is charged at admission, so the Job controller never creates one and `active` is 0 |
| Evidence | `VcpuLimitExceeded` / `MaxSpotInstanceCountExceeded` on the fleet request | a `FailedCreate` Event whose message contains `exceeded quota` |
| Lives in | Service Quotas, read with `get-service-quota` | the cluster, authored in Terraform/Helm |
| Fix | a quota increase request, and a closed case is not a granted one | resize the quota, or size the workload to it |
| Owned by | this skill | the infra-platform skill |

**Whether a pod exists at all is the discriminator**, not the symptom — both
report `Pending` work and both end in the dispatcher writing a *capacity wait*
on the `task_instance` row (`er_k8s::classify::pre_start` →
`PreStartVerdict::CapacityWait`), which deliberately spends no launch, because a
starved account is the operator's problem and never the tenant's. The verdict is
therefore the same either way; only the evidence underneath it differs.

**Read the applied value; never answer this from a document, including this
one.** The quota is live account state that no generator here can derive, and
the failure has already happened twice in both directions: an increase request
was `CASE_CLOSED` *without being granted*, and this skill asserted the
requested figure as the applied one for two weeks on the strength of that closed
case. A closed case is not a granted one.

```
aws service-quotas get-service-quota --service-code ec2 \
  --quota-code L-34B43A08 --region us-east-2 --query 'Quota.Value'
aws service-quotas list-requested-service-quota-change-history-by-quota \
  --service-code ec2 --quota-code L-34B43A08 --region us-east-2 \
  --query 'RequestedQuotas[].[Status,DesiredValue]'
```

Then do the arithmetic against what you read, because the ceiling is shared with
everything already running. Worked example at the AWS default of 8: the
always-on `app` and `system` nodes hold 2 vCPU each, leaving 6 vCPU of headroom —
less than one 8-vCPU `runner-sm` node, so no runner node of either class can
place at all. Substitute the value the command returned; if it exceeds the sum
of the always-on nodes plus the node class you are asking for, the quota is not
your problem and the `Pending` pod is something else.

**A healthy IAM says nothing about cluster access — the two are separate
planes.** A read-only SSO role (`er-readonly` / PlatformReadOnly) can
authenticate against every AWS API and still get **401 Unauthorized from the
EKS cluster itself**, because Kubernetes authorization is not IAM
authorization: without an EKS *access entry* covering the role's ARN, the
cluster refuses the token no matter how healthy the AWS side looks. The
discriminator is `aws sts get-caller-identity`: if it fails, the SSO token
itself is the problem (run the login again); if it answers but `kubectl` 401s,
the missing piece is the access entry. The grant for the read-only role lives
in the `eks` module's `viewer_principal_arns`
(`infra/terraform/envs/dev/eks.tf`), associated with `AmazonEKSViewPolicy` at
cluster scope — which reads no Secret contents, matching the AWS-side
ReadOnlyAccess posture (added by #839 / PR #844 after a session lost time to
exactly this misdiagnosis). Verify with
`kubectl --context er-dev-ro -n ci get pods` returning instead of 401ing.

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
anything declared without touching the account. For live state, a read-only
`describe`/`list` under `AWS_PROFILE=er-readonly`. Not the MCP server: under
`--read-only` it carries no tool that reaches the account.

**A pod cannot reach AWS.** Check for an `aws_eks_pod_identity_association`
covering its exact (cluster, namespace, service account) triple before looking
at the policy. Adding a second role for a pod that already has one is the
common wrong turn.

**A pod is stuck `Pending`.** Check the vCPU quotas above before anything else.

**"Why did the bill move?"** Budgets only tell you *that* it did. The CUR 2.0
export with split cost allocation is the only thing that answers *why*, queried
either through the `er-cost` Athena workgroup or by reading the delivered
Parquet with DuckDB — see the cost reference for which to pick. If the period
predates tag activation, the answer may not exist.

**A new AWS resource is needed.** Write Terraform. Then `terraform fmt
-check -recursive` and a credential-free `init -backend=false && validate`,
which is what CI runs. Do not apply from an agent session.

**Reaching for the `aws` CLI to change something.** Prefer Terraform. If a
one-off genuinely is right, expect the Bash guard to deny it outright or ask
first — that is the guard working, not a bug.
