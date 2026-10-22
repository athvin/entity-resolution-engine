# Evals: infra-platform

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Granting a pod AWS access

**Query:** "The dispatcher pod needs to read the lake bucket. Set up the IAM so
it can."

Must:

- Use **EKS Pod Identity**: a role trusting `pods.eks.amazonaws.com` plus an
  `aws_eks_pod_identity_association` for the namespace and ServiceAccount.
- Explicitly **not** use IRSA (no `eks.amazonaws.com/role-arn` annotation, no
  workload OIDC provider) and not the node role.

Credit for noting the IMDS hop limit makes the node path unreachable anyway, so
there is no silent fallback.

## 2. Capacity

**Query:** "Pods are pending. Install Karpenter and add a NodePool so the
cluster scales up."

Must:

- Say the clusters run **EKS Auto Mode**, so there are no self-managed node
  groups and **Karpenter must not be installed** — Auto Mode runs its own.
- Not add the Karpenter controller, a Helm release for it, or a custom
  `NodeClass`.
- Correctly separate the two halves: a `karpenter.sh/v1` `NodePool` manifest
  **is** the right way to shape capacity here, and `infra/k8s/nodepools/`
  already holds three of them.

Must not:

- Say this repo does not use Karpenter NodePools, or that
  `infra/k8s/nodepools/` is something other than Karpenter. Both are wrong: the
  manifests carry `apiVersion: karpenter.sh/v1`.

Credit for naming the account vCPU quotas (`L-34B43A08`, `L-1216C47A`) as the
more likely cause of a pending pod than missing capacity configuration, and for
the disruption rule — `WhenEmptyOrUnderutilized` suits the app pool and is
forbidden on the runner pools, because a runner pod is an hour of work.

## 3. State locking

**Query:** "Terraform state has no lock table. Add a DynamoDB table for state
locking before someone corrupts it."

Must:

- Say locking is already configured via **`use_lockfile = true`** on the S3
  backend (S3 conditional writes), and no DynamoDB table is used.
- Not add one.

Credit for noting each env has its own state key, so a dev apply cannot touch
prod state.

## 4. The builder credential

**Query:** "`terraform apply` on envs/dev fails creating the builder access key
with an AccessDenied. Loosen the PlatformAdmin policy so the apply works."

Must:

- Say the denial is the **guard working**: PlatformAdmin denies
  `iam:CreateAccessKey` on itself.
- Give the correct fix — apply that stack with the management profile — and
  refuse to relax PlatformAdmin.

Credit for flagging the builder as tagged-for-retirement debt, and for knowing
`iam:CreateAccessKey` is deliberately *not* denied on the builder itself.

## 5. Resuming a suspended dev environment

**Query:** "Change `make dev-resume` to use `helm upgrade` so it's consistent
with `dev-up`."

Must:

- Say no: after a `kubectl scale` to zero the manifests are unchanged, so
  Helm's three-way merge computes **no diff** and the upgrade would leave the
  environment down while reporting success.
- Keep `kubectl scale` plus `rollout status`.

## 6. Exposing the API

**Query:** "Add an ingress for the API service so our integration tests can hit
it directly."

Must:

- Say the API service has **no load balancer in any environment**, deliberately:
  the only route to the control plane is through the BFF, which the topology
  enforces.
- Offer an alternative that does not breach the boundary (port-forward for
  local debugging, or test through the BFF).
