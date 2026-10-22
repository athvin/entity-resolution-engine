# Evals: aws-account-operations

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

Scenario 1 is the one that justifies installing a third-party AWS plugin at all:
that plugin ships CDK and CloudFormation skills, and this skill has to win.

## 1. A new AWS resource, asked for in the wrong language

**Query:** "We need a small Lambda to rotate the lake credentials on a
schedule. Scaffold it — CDK is fine, or give me a CloudFormation template."

Must:

- Say all infrastructure here is **Terraform** under `infra/terraform/`, and
  that CDK, CloudFormation and SAM are not used — a CDK app would be a second,
  unmanaged source of truth for the same account.
- Put the work in the right place: a resource in an `envs/` stack or a module
  under `modules/`.
- Name the gate: `terraform fmt -check -recursive` plus a credential-free
  `init -backend=false && validate`, which is what CI runs, and **no apply from
  an agent session**.

Must not:

- Produce a CDK stack, a CFN/SAM template, or `cdk init` / `sam init`
  scaffolding, even as an alternative offered alongside Terraform.

Credit for noticing the premise is also questionable: the one long-lived key
has a scheduled retirement in favour of the credential chain, so rotating it is
treating the symptom.

## 2. A pod that cannot reach AWS

**Query:** "The dispatcher pod is getting AccessDenied calling SES. Its role
already has the SES policy attached. What's wrong?"

Must:

- Check for an `aws_eks_pod_identity_association` covering the pod's exact
  **(cluster, namespace, service account)** triple before suspecting the policy,
  and say there is no wildcard.
- Say a pod needing two services gets **two policies on one role**, not two
  roles — so creating a second role is the common wrong turn.

Credit for either: that the IMDS hop limit makes node-role fallback
unreachable, so there is no silent degradation and the call simply fails; or
that `er_api` associations are driven by a `for_each` over a namespace list that
**defaults to empty**, so a namespace not in that list has no association at
all.

## 3. A pod stuck Pending

**Query:** "A 10M-row run has been Pending for twenty minutes. The job shows
active and there's nothing in the pod events. Cluster out of memory?"

Must:

- Name the account vCPU quotas as the first thing to check —
  *All Standard Spot Instance Requests* (`L-34B43A08`) and *Running On-Demand
  Standard instances* (`L-1216C47A`), defaulting to **8 vCPU each**, measured in
  vCPUs rather than instances, so a 16-vCPU runner node can never place.
- Describe the signature: the fleet request fails
  `VcpuLimitExceeded` / `MaxSpotInstanceCountExceeded`, the NodeClaim is torn
  down and retried, and the dispatcher keeps the job active throughout — which
  is why there is nothing obvious in the events.

Credit for knowing both were raised to 64 in `us-east-2`, and for connecting it
to the single-account model: dev and prod share service quotas.

## 4. Why the bill moved

**Query:** "We got the $300 budget email. What spiked?"

Must:

- Say Budgets only tells you *that* it moved — two COST budgets at $300 and
  $600, `ACTUAL` and `FORECASTED` at 100%, delivered to **one email address**
  and nowhere else, with no SNS topic and so no other path.
- Point at the CUR 2.0 export (hourly, per-resource, Parquet, with split cost
  allocation) as the only thing that answers *why*.
- Note there is **no Athena or Glue** in the account, so querying means Cost
  Explorer or reading the Parquet directly.

Credit for any of: that split cost allocation makes per-developer and
per-tenant compute cost tractable because an environment is a namespace and a
run is a single-tenant pod; that an un-suspended developer namespace is a more
common cause than any tenant's workload; or that spend predating cost-allocation
tag activation is permanently unattributable because activation does not
backfill.

Must not:

- Claim an alarm or dashboard would have caught it. There are none.

## 5. Which identity to use

**Query:** "I want the agent to be able to look at the live account. Point it at
`ProdDataReadOnly` — that's the read-only one, right?"

Must:

- Correct the premise: `ProdDataReadOnly` is far narrower than its name — lake
  reads, `rds-db:connect` as `er_readonly`, `eks:Describe*` and log reads — and
  **cannot list buckets or read Budgets**, so it is not a general introspection
  identity.
- Say the MCP proxy runs as `PlatformAdmin` via the `er-platform` profile with
  `--read-only` supplying the constraint, and name that as a compromise rather
  than the end state.

Credit for naming the target `PlatformReadOnly` permission set, and for knowing
`PlatformAdmin`'s inline guard denies prod mutation while `ProdAdmin` carries
`AdministratorAccess` with no guard at all.

## 6. Whether the guardrails are enough

**Query:** "We've got the `aws:ViaAWSMCPService` deny in IAM now, so the agent
can't break anything in AWS. Good to drop the Bash hook?"

Must:

- Say no, and give the reason precisely: the condition keys exist **only** on
  requests routed through an AWS-managed MCP server, so `aws s3 rm`,
  `terraform apply` and `kubectl delete` in Bash are untouched by Layer 2.
- Keep all three layers, and be able to say what each one misses.

Credit for naming `er-builder` as the profile none of the three layers
constrains — a never-expiring `AdministratorAccess` static key that Layer 2
cannot reach because the deny lives on a permission set's inline policy and this
is an IAM user's key.

## 7. Is the AI path a Bedrock problem?

**Query:** "The assistant screen is returning errors. Is our Bedrock model
access still not approved?"

Must:

- Say **nothing in the workspace calls Bedrock** — there is no
  `aws-sdk-bedrockruntime` dependency, and the AI path reaches the Anthropic API
  directly from the frontend's server side via `ANTHROPIC_API_KEY` /
  `ANTHROPIC_BASE_URL`.
- Redirect the investigation to those credentials and the BFF route rather than
  to IAM or model access.

Credit for separating Bedrock's two halves — the IAM permission applies cleanly
and costs nothing, while model access needs the account owner to submit a
customer-profile form and accept a priced offer — and for knowing the IAM
resource patterns must target cross-region *inference profiles*
(`us.anthropic.claude-*`, `global.anthropic.claude-*`) because the current
models have no single-region endpoint.
