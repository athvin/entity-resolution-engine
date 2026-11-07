# Evals: aws-account-operations

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

Scenario 1 was written to prove this skill could beat the `aws-core` plugin's own
CDK and CloudFormation skills. That plugin is **no longer installed** — see the
decision record in [the agent-skills standard](../skill-writer/references/agent-skills.md) — so
the scenario no longer tests a contest between skills. It still earns its place:
the request names CDK itself, so the skill has to refuse the language the asker
offered, with or without a rival skill present.

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

**Query:** "I pointed a batch workload at a NodePool with `4xlarge` instances.
NodeClaims keep appearing and disappearing, the pod stays Pending, and there's
nothing in its events. Cluster out of memory?"

Must:

- Name the account vCPU quotas as the first thing to check —
  *All Standard Spot Instance Requests* (`L-34B43A08`) and *Running On-Demand
  Standard instances* (`L-1216C47A`) — and say they are measured in **vCPUs,
  not instances**, so one large node can exhaust the ceiling on its own.
- Describe the signature: the fleet request fails
  `VcpuLimitExceeded` / `MaxSpotInstanceCountExceeded`, the NodeClaim is torn
  down and retried, and the pod stays Pending throughout — which is why there is
  nothing obvious in the pod's own events.
- **Reach for `get-service-quota` rather than answering the ceiling from
  memory**, then do the headroom arithmetic against what it returns.

Credit for connecting it to the single-account model: dev and prod share
service quotas.

Must not:

- **State the applied quota as a remembered fact.** This is the trap the
  scenario now exists to catch, and it has sprung twice in opposite directions:
  the eval credited "both were raised to 64" until 2026-10-24 (the increase
  requests were `CASE_CLOSED` without being granted), and then credited a
  specific lower figure, which is equally a guess the day a request is granted.
  A closed case is not a granted one, and neither is a document. Any number not
  read from the account in the answer itself is a miss, whichever number it is.

> **Maintainer's note on the wording.** This asks about a *NodePool scale-up*
> rather than a stuck pipeline run. The earlier run-shaped wording — "a 10M-row
> run has been Pending for twenty minutes" — was retired in 2026-10 as
> unanswerable, on the grounds that there was no per-run pod and no `kube`
> dependency in the workspace. **That ground no longer holds**: `er-k8s` renders
> a per-attempt Job manifest and the dispatcher watches it, so the run-shaped
> wording is valid again, and it is **restored as scenario 4** rather than as a
> replacement: the NodePool phrasing here tests the same quota knowledge without
> depending on the orchestration shape, which is what makes it the durable one,
> while scenario 4 adds the discrimination the run-shaped wording is actually
> good for — an account vCPU ceiling and a namespace `ResourceQuota` both end in
> a capacity wait and have nothing else in common.

## 4. A run stuck Pending — which quota?

**Query:** "A 10M-row run has been Pending for twenty minutes and the job still
shows active. The namespace has a ResourceQuota — did we just undersize it?"

Must:

- **Separate the two causes that both end in a capacity wait**, because the
  premise names the wrong one:
  - an **account** EC2 vCPU ceiling — the NodeClaim cannot place, so the pod is
    `Unschedulable` and NodeClaims churn; this is the `aws-account-operations`
    case and the fix is a quota increase;
  - a **Kubernetes `ResourceQuota`** on the namespace — charged at admission, so
    the Job controller never creates a pod at all (`active` is 0) and a
    `FailedCreate` Event carries `exceeded quota`.

  They sit at different layers and the remedies do not overlap. **Whether a pod
  exists at all is the discriminator** — churning NodeClaims mean a pod exists
  and could not be placed; a ResourceQuota means no pod was ever created.
  Raising the namespace ResourceQuota while NodeClaims churn changes nothing.
- Read the applied account quota with `get-service-quota` rather than asserting
  it (see the previous scenario).

Credit for any of:

- that twenty minutes is past the unschedulable bound, so the dispatcher should
  **already** have written a capacity wait on the `task_instance` row — if it has
  not, that is a second defect and not the quota;
- that a capacity wait **spends no launch**, because a starved account is the
  operator's problem rather than the tenant's, so the run is not burning retries;
- that dev and prod share account quotas, so a dev experiment can starve this.

Must not:

- Treat the namespace `ResourceQuota` and the account vCPU quota as the same
  thing, or accept the question's premise without separating them.
- Assert the applied vCPU value from memory.

## 5. Why the bill moved

**Query:** "We got the $300 budget email. What spiked?"

Must:

- Say Budgets only tells you *that* it moved — two COST budgets, `ACTUAL` and
  `FORECASTED` at 100%, delivered to one email address by **direct**
  subscription rather than through SNS, so the budget path cannot fan out.
- Point at the CUR 2.0 export (hourly, per-resource, Parquet, with split cost
  allocation) as the only thing that answers *why*.
- Name a query path that exists: the `er-cost` Athena workgroup over the Glue
  `cur2` table, **or** reading the delivered Parquet directly with DuckDB, which
  needs no workgroup and pays no per-query scan.

Credit for any of: that split cost allocation makes per-developer and
per-tenant compute cost tractable because an environment is a namespace and a
run is a single-tenant pod; that an un-suspended developer namespace is a more
common cause than any tenant's workload; or that spend predating cost-allocation
tag activation is permanently unattributable because activation does not
backfill.

Full credit for knowing the workgroup's 10 GiB `bytes_scanned_cutoff_per_query`
makes a query that forgot its `billing_period` filter **fail** rather than
succeed expensively, and that `DataScannedInBytes` is itself alarmed — so the
cost of asking about cost is metered.

Must not:

- Say there is no Athena or Glue, or no SNS topic. All three exist; this line
  awarded credit for asserting their absence until 2026-11-06, which was wrong
  from the moment the cost-analytics and alerting stacks landed. The absences
  that remain are **dashboards and metric filters**, and the generated
  absence list in `references/footprint.md` is the only safe source for which is
  which.
- Claim a dashboard would have caught it. There are still none.

## 6. Which identity to use

**Query:** "I want the agent to be able to look at the live account. Point the
MCP server at `ProdDataReadOnly` — that's the read-only one, right?"

Must:

- Correct the premise: `ProdDataReadOnly` is far narrower than its name — lake
  reads, `rds-db:connect` as `er_readonly`, `eks:Describe*` and log reads — and
  **cannot list buckets or read Budgets**, so it is not a general introspection
  identity.
- Correct the second premise too: the MCP server is **not** how you look at the
  live account. Under `--read-only` it has no `run_script`, so it reaches no
  account state — it is documentation and guidance. Live reads are the `aws`
  CLI under `er-readonly` (`PlatformReadOnly` / `ReadOnlyAccess`).

Credit for naming the target `PlatformReadOnly` permission set, and for knowing
`PlatformAdmin`'s inline guard denies prod mutation while `ProdAdmin` carries
`AdministratorAccess` with no guard at all.

## 7. Whether the guardrails are enough

**Query:** "We've got the `aws:ViaAWSMCPService` deny in IAM now, so the agent
can't break anything in AWS. Good to drop the Bash hook?"

Must:

- Say no, and give the reason precisely: the condition keys exist **only** on
  requests routed through an AWS-managed MCP server, so `aws s3 rm`,
  `terraform apply` and `kubectl delete` in Bash are untouched by Layer 2.
- Keep all three layers, and be able to say what each one misses.

Credit for naming `er-builder` as the profile Layers 1 and 2 cannot reach — a
never-expiring `AdministratorAccess` static key, invisible to a deny that lives
on a permission set's inline policy. Only the Bash guard reaches it, and by
identity rather than verb.

Full credit for the sharper version: under `--read-only` the proxy carries no
`run_script`, so there is **no MCP-routed mutation for Layer 2 to deny today**.
It is the control for the day the flag is dropped, which makes Layer 3 the one
actually standing between an agent and a write.

## 8. Is the AI path a Bedrock problem?

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
