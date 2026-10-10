# Plan to AWS — the ordered build plan, with acceptance criteria

*The execution companion to [infrastructure.md](infrastructure.md) (third revision). That
document holds every design decision and its rationale; this one holds only **order** and
**definition of done**. Each task cites the section that designed it — if a task and the
design disagree, the design doc wins and this file is stale. Written 2026-10-03.*

Tasks are checkboxes so this file doubles as the tracker. A task is **done** when every
acceptance criterion is observed — not when the Terraform applies. That rule is inherited
from infrastructure.md §17: a green apply proves nothing this plan cares about.

---

## Where we start, where we end

**Start:** Task 0.1 — the AWS Organization. Nothing else can exist first, and Phase 0
carries everything that cannot be backfilled (audit trails, cost attribution, tamper
evidence). Two long-lead items start in parallel the same week: the domain purchase (gate
D2) and the SSO provider registrations (Track S), because both sit on other people's review
queues.

**End:** Task 4.4 — both static credentials retired and the restore test recorded. At that
point: a developer runs `make dev-up`, refreshes real prod data into their namespace, and
reaches it over the tailnet; prod serves customers behind WAF with SSO login; no long-lived
access key exists in any account; and the audit trails have been accumulating since the
first week, which is what makes a SOC 2 observation window possible at all.

**The dependency shape** (critical path down the left, parallel tracks right):

```
Phase 0  foundations ──────────────┬─► Track A (app images/changes, after 0.6)
   │  gates: D1 Auto Mode,         └─► Track S (SSO registrations, after D2)
   │         D2 domain
Phase 1  cluster + substrate ──────── Track A merges here (1.8 needs images)
   │
Phase 2  dev env + Job launcher ───── Track S merges here (S2 needs a running app)
   │
   │  gate: first prod traffic
Phase 3  prod
   │
Phase 4  hardening + audit run-up  ◄── end state
```

---

## Decision gates

These are not tasks; they are answers the plan blocks on. Resolve each before the phase
that consumes it.

- [x] **D1 — Auto Mode or self-managed Karpenter** (§5.5, §18.3). **Resolved 2026-10-10:
  Auto Mode.** `do-not-disrupt` is honoured across consolidation policies and NodePools
  expose `consolidationPolicy: WhenEmpty` + `consolidateAfter`; the ~12%-of-on-demand fee
  (charged even on Spot) is ~$10–20/mo at this footprint, with a revisit trigger at
  ~$50/mo recorded in §5.5. Tasks 1.2/1.3 build the Auto Mode branch — no system node
  group, no Karpenter controller to operate; snapshot-prebake (§11.1 rung 3) is off the
  table, so image-pull mitigation stops at SOCI + the image split.
- [x] **D2 — domain** (§18.8). **Resolved 2026-10-10: `dupezero.com`, Cloudflare Registrar.**
  Registrar pins the apex to Cloudflare nameservers, so Cloudflare is the DNS plane —
  external-dns (Cloudflare provider, unproxied records), ACM validation and SES DKIM
  CNAMEs via the Terraform `cloudflare` provider. Track S and 1.4 are unblocked.
- [ ] **D3 — progress reporting** (§6.3, §18.5). The recommendation (runner writes its own
  progress to Postgres) stands unless overturned. **Confirm before 2.3 is started**, since
  the Job launcher is built around the answer.
- [ ] **Prod gate** — Phase 3 starts when there is a first paying/piloting tenant, not
  before (§15). Deferral is the design, not slippage.

---

## Phase 0 — Foundations (start here)

*(Rev 4, 2026-10-10: executed against the single-account topology — infrastructure.md
§3 records the collapse and its trade. Checkboxes below reflect actual state.)*

Everything in this phase is verifiable with no cluster running (§15). The ordering rule
inside the phase: **0.6 (cost tags) before any billable resource exists**, because
activation takes up to 24h and never backfills (§16.1).

- [x] **0.1 The account baseline** (§3) — account `er` 797781631727 under Organization
  `o-bqzb54i69b` (kept for Identity Center; otherwise inert).
  - [x] Root MFA on; zero root access keys (`AccountAccessKeysPresent: 0`).
  - [x] Account-level S3 Block Public Access on (§5.6).
  - The former `nonprod` member account (660360495170) is parked empty at the org root.

- [x] **0.2 Terraform bootstrap — two stacks** (§14)
  - [x] State bucket: versioned, SSE-KMS under its own CMK, `use_lockfile` locking.
  - [x] Both bootstrap stacks' state migrated into the bucket; plans clean.

- [x] **0.3 Identity Center** (§8.1)
  - [x] PlatformAdmin 8h / ProdDataReadOnly 4h / ProdAdmin 1h permission sets; access by
    group membership (`developers`, `prod-admins`); zero IAM users for humans.

- [x] **0.4 The two guard denies** (§8.2 — rev 4: inline on the permission sets, no SCP)
  - [x] `DenyProdDataMutation` + `DenyProdIamEscalation` ride `PlatformAdmin`.
  - [x] §17 Phase 0 probe run (2026-10-10): under `PlatformAdmin`, PutObject on an
    `er-prod-*` bucket and `iam:CreateRole er-prod-app-probe` both fail with
    "an explicit deny in an identity-based policy"; under `ProdAdmin` both succeed.
    **Re-run after any permission-set change** — the guard now lives in that layer.

- [x] **0.5 Audit trail, Object Lock, Config** (§8.6, §15)
  - [x] CloudTrail `er-org-trail` logging, multi-region, log-file validation on,
    delivering to the compliance-mode Object-Locked bucket (400d) under its own CMK.
  - [ ] First digest delivered and `aws cloudtrail validate-logs` passes (waiting on delivery).
  - [x] The control test (2026-10-10): under `PlatformAdmin`, deleting an archive object fails with AccessDenied.
  - [x] AWS Config recording, narrow scope.

- [x] **0.6 Cost attribution — before anything billable** (§16.1)
  - [x] CUR 2.0 export (hourly, Parquet, per-resource, Split Cost Allocation Data) delivering to S3.
  - [ ] `Project`, `Environment`, `ManagedBy`, `Developer` activated once they surface in
    billing (~24h after first tagged resource).
  - [ ] Deferred check: first CUR file contains split-allocation columns.

- [x] **0.7 GitHub OIDC + ECR** (§11, §17 Phase 1)
  - [x] `er-pipeline`/`er-api`/`er-web` repositories: tag immutability, scan-on-push,
    last-30 lifecycle. Single registry — replication dissolved with the accounts.
  - [x] OIDC provider + `er-ci-ecr-push` with `sub` pinned to `main`; CI `push-image` job.
  - [x] Probe (2026-10-10): first main push landed — `er-pipeline:a2dccbf…` in ECR via OIDC
    (after pinning the trust policy to GitHub's immutable subject claims). Remaining:
    re-push-same-tag rejection and the PR-branch assume-fails negative probe.

- [x] **0.8 Branch protection + budget alarms** (§8.6, §16)
  - [x] Direct push to `main` rejected; PR without review cannot merge (1 review + all 36
    checks required; `enforce_admins` off as the conscious solo-operator bypass).
  - [x] Budget alarms at $300 and $600 (ACTUAL + FORECASTED, email).
  - [ ] Fire a test notification and see it arrive.

**Phase 0 exit:** all §17 Phase 0 probes pass, and the two "cannot backfill" systems
(trails, cost attribution) have each produced their first real artifact.

---

## Track A — application changes (parallel, start any time after 0.7)

All from §14.1. No cluster required; CI is the harness. These merge into Phase 1 at 1.8.

- [ ] **A.1 Two new images** — `docker/Dockerfile.api` (slim: no dbt, no MinIO), `frontend/Dockerfile` (Next standalone, Node 22)
  - CI builds and pushes all three images; `make check-all` passes, including `test_dockerfile_contract.py` and `test_dbt_profiles.py` untouched.
  - Both new images run as non-root (`docker run --rm <img> whoami` ≠ root) (§5.6).
- [ ] **A.2 `ERSERVER_TENANT_DB_PREFIX`** (§7.2) — default `er_`, behaviour bit-for-bit unchanged without the var; a unit test proves both the default and an override.
- [ ] **A.3 Lake prefix normalization** (§7.3) — one layout across CI compose, dev stack, and settings default; the integration tier passes.
- [ ] **A.4 FastAPI docs off outside dev** (§4.1) — `/docs`, `/redoc`, `/openapi.json` return 404 under prod settings; still served in dev.
- [ ] **A.5 Non-root `USER` in `docker/Dockerfile`** — the compose `pipeline` service (whose command is pytest) still passes the full integration tier.

**Track A exit:** a full integration-tier run (`make check-all` plus the 2h08m single-session
pass) is green on the branch that carries all five — that tier, not the fast tiers, is what
catches cross-module leaks here.

---

## Phase 1 — Cluster and substrate (needs: Phase 0, D1, D2)

- [ ] **1.1 Network** (§4) — *applied 2026-10-10: er-dev VPC /16, 3 AZs, single NAT, S3 gateway endpoint on both route tables; API inventory confirms zero interface endpoints. The private-instance egress probe remains.*
  - From a private-subnet test instance: S3 reachable with the NAT route removed (gateway endpoint carries it); general egress via NAT; **zero interface endpoints exist**.
- [ ] **1.2 EKS cluster** (§5; shape depends on D1)
  - `kubectl` works via Identity Center; the `view`-only group can read but cannot read Secrets (`kubectl auth can-i get secrets` → no).
  - All addons healthy; cluster version is the newest minor, pinned in Terraform.
- [ ] **1.3 Nodes + hardening** (§5, §5.1, §5.6; shape depends on D1)
  - `app` NodePool provisions; system pods scheduled.
  - From a non-hostNetwork pod: `curl -m 1 169.254.169.254` **times out** (IMDSv2 hop limit 1).
  - A privileged test pod is rejected in a namespace labeled `baseline` (PSS enforcing).
- [ ] **1.4 Edge** (§4.1)
  - A test Ingress gets a Cloudflare DNS record (external-dns, unproxied) and the wildcard cert; reachable over the tailnet; **connection fails from a non-tailnet network** — test both directions.
  - One ALB serves multiple test Ingresses via `group.name`.
- [ ] **1.5 EFS** (§7.4)
  - Mounts from a test pod through a namespace access point; write on one pod, read on another.
  - AWS Backup default plan attached; first recovery point exists after the first daily window.
- [ ] **1.6 Secrets** (§9, §9.1)
  - ExternalSecret syncs a Secrets Manager entry into the namespace; editing the secret triggers a rolling restart of an annotated test Deployment — observe the restart, don't trust the annotation.
  - None of the `frontend/dev/env.sh` committed values appear in any cloud secret (§8.5) — grep the synced Secrets for the known strings.
- [ ] **1.7 SES** (§7.5)
  - Domain identity verified (DKIM CNAMEs resolve); a pod sends a test mail via `ERSERVER_SMTP_URL` to a verified address and it arrives with DKIM pass.
  - Production-access request **filed** (not granted — filing is the DoD; dev runs in sandbox indefinitely).
- [ ] **1.8 Observability floor** (§10.6 Phase 1)
  - Collector gateway and log DaemonSet healthy; a test pod's stdout appears in CloudWatch.
  - With the er-api image from Track A deployed as a smoke test: RED metrics for `/healthz` visible via auto-instrumentation.

**Phase 1 exit:** a smoke `er-api` pod is reachable at a tailnet hostname with TLS, its
logs and metrics are visible, and its secrets came from Secrets Manager.

---

## Phase 2 — Developer environments and the Job launcher (needs: Phase 1, Track A, D3)

The heart of the migration. 2.3 is the largest single piece of work in the plan.

- [ ] **2.1 `er-platform` chart** (§12, §14)
  - API, dispatcher, web deploy; dispatcher has `replicas: 1`, `strategy: Recreate`, and an init container gating on `/healthz` — kill the API and watch the dispatcher init **block** (§2 boot order).
  - The drizzle pre-upgrade hook Job ran (`drizzle.__drizzle_migrations` matches `_journal.json`); `ensure_schema` completed on API boot.
  - API and dispatcher pods both mount EFS at config root and drop root (§6.2) — `exec` and `touch` a file from the API, read it from the dispatcher.
- [ ] **2.2 `er-dev-namespace` chart + lifecycle targets** (§12, §17 Phase 2)
  - `make dev-up DEV=alice` → login at `alice.dev.<domain>` over the tailnet, create an org, submit `run_all_full`, run completes.
  - `er lake maintain` succeeds — the `max_locks_per_transaction=1024` flag took effect (the operation that fails at 64).
  - `make dev-suspend` → Karpenter reclaims the app node and `pg_stat_activity` on the (stopped) DB path shows nothing held; resume → golden-record counts and snapshot history unchanged (PVC survived).
  - From alice's namespace, a connection to bob's Postgres service **fails** — NetworkPolicy enforcing, which proves the CNI policy agent is actually on (§8.5).
  - `make dev-down` → PVC gone, S3 prefix empty, secrets deleted.
- [ ] **2.3 Runner NodePools + the Kubernetes Job launcher + its coupled set** (§5.1, §5.3, §6, §10.2, §15)
  - One Job per run; on a deliberate config error (engine exit 2), **exactly one pod** is ever created (`backoffLimit: 0` held).
  - `df -h /app/.tmp` inside a runner shows instance-store NVMe, not the root filesystem; the pod carries `requests.ephemeral-storage` and the `emptyDir` sizeLimit.
  - Runner pods carry `karpenter.sh/do-not-disrupt`; the node empties ~30s after the run.
  - Progress: visible in the job-detail UI during a run, **and still updating after the dispatcher is restarted mid-run** (the D3 design's whole point).
  - One trace spans BFF → API → queue wait → Job pod → every stage → dbt, including the final stage and exit status (`force_flush` held).
  - Per-run sizing rides the job row, not `orgs.env`; the Job's cpu/memory/threads/ephemeral match the record-count class (§6.5).
  - **Measurement artifact:** cold-node pull time recorded in §18.16 — it decides whether §11.1's image-split rung happens (spawn it as a follow-up task if pull time is unacceptable).
- [ ] **2.4 Idle-suspend CronJob** (§5.2)
  - A namespace with `last_seen_at` > 4h old and no jobs suspends on the next tick; a namespace with a **running job does not**, regardless of session age. Test both.
- [ ] **2.5 The two paranoia checks** (§17 Phase 2)
  - Grep a completed run's logs for a seeded name, email, and phone — all three absent (§10.4's rule, which §8.1's log grant depends on).
  - A spill-heavy run (10M-scale fixture) completes without eviction (§5.3 sizing is right).

**Phase 2 exit:** all nine §17 Phase 2 checks pass in a fresh namespace created from
nothing by `make dev-up`.

---

## Track S — SSO (registrations after D2; implementation after 2.1)

Design: [frontend-design.md](frontend-design.md) §2.4. Zero erserver changes.

- [ ] **S.1 Provider registrations** (start early — all three have human lead time)
  - Google: client created, consent-screen/branding verification **submitted**; Entra: single registration, `AzureADandPersonalMicrosoftAccount`, v2 tokens; Salesforce: one Connected App registering `openid email profile api refresh_token`.
  - Redirect URIs registered exactly (no wildcards exist): each dev hostname + the prod placeholder.
  - Three client-ID/secret pairs landed in the `er/{env}/{ns}/erweb` secret (infrastructure §9).
- [ ] **S.2 Implementation**
  - `identities` table migrated; `password_hash` nullable; `PUBLIC_PATHS` + `/api/auth/sso`.
  - Invite accepted via "Continue with Google" creates user + identity + membership and **no password**; the same user then logs in via Google into the ordinary `er_session` cookie.
  - SSO login with no identity row and no invite is **rejected** — and specifically: create a password user, then SSO-login with the same email at the IdP → rejected, not linked (the no-auto-link rule, tested as a negative).
  - Subjects keyed per the §2.4 table (`sub` / `tid:oid` / `orgId:userId`) — assert in a unit test, not by inspection.
  - Password login unchanged; a signed-in user links a provider from settings and the row appears.
  - The dangling `/reset` page exists and the emailed reset token round-trips.
- [ ] **S.3 Onboarding copy** — the Salesforce admin-install requirement (Sept 2025 restriction) documented where a customer admin will actually see it.

**Track S exit:** a fresh invite is accepted end-to-end with each of the three providers
against a dev namespace, and the negative tests (no invite, email collision) both reject.

---

## Phase 3 — Prod (needs: the prod gate, Phase 2)

- [ ] **3.1 Prod network + EKS + Aurora** (§5, §7.1)
  - Aurora Serverless v2 multi-AZ with the cluster parameter group; `er lake maintain` against Aurora succeeds (the `max_locks` proof, again — different mechanism than dev's `-c` flag).
  - Every DSN carries `sslmode=verify-full` and connects — which first requires the CA bundle in the images (§18.15; expect an image change).
  - 35-day PITR confirmed on, from the API.
- [ ] **3.2 Prod lake + scoped CloudTrail** (§7.3)
  - SSE-KMS under the prod CMK; TLS-only bucket policy (plain HTTP probe fails).
  - Data events: a read on the prod prefix produces an event; a write does not; a nonprod-bucket read does not. Probe all three.
  - Lifecycle: noncurrent versions 30d, incomplete multipart 7d — read back from the API.
- [ ] **3.3 Prod edge** (§4.1)
  - Internet-facing ALB + WAF; a synthetic single-IP flood against `/api/auth/login` gets rate-limited at the configured threshold while a second IP's legitimate login succeeds.
- [ ] **3.4 Refresh path + spot resilience** (§13, §17 Phase 3)
  - `make refresh-dev` completes; dev golden-record counts reconcile against prod.
  - Terminate a runner node mid-job: the job requeues with `--resume` and completes — this is the test that validates the spot pricing assumption in §16.
- [ ] **3.5 Prod deploy path** (§14.2)
  - A deploy referencing a tag is refused; a digest not yet replicated blocks; the replicated digest deploys. All three cases exercised.
  - Re-run the full §17 Phase 0 SCP probes against the now-existing prod lake.

**Phase 3 exit:** a real tenant's run completes in prod, and `refresh-dev` has moved its
data into a developer namespace using only the designed path.

---

## Phase 4 — Hardening and the audit run-up (end state)

- [ ] **4.1 Retire both static credentials** (§9.2, §9.3, §18.17 — ship together)
  - S3: DuckDB `credential_chain` + boto3 default chain via IRSA; `tests/unit/test_dbt_profiles.py` still holds the three config sites identical.
  - Email: `mailer.py` ported to the SES API under IRSA; invites and password resets still deliver.
  - The check that makes it real: `aws iam list-users` / `list-access-keys` across both workload accounts returns **zero** service users — §8.1's claim becomes unconditional.
- [ ] **4.2 Per-tenant runner ServiceAccounts** (§6.4) — a runner for tenant A, given tenant B's prefix explicitly, gets AccessDenied from S3.
- [ ] **4.3 Observability completion** (§10.6 Phase 4) — dashboards for the backend-design §14 metric list; alerts on queue depth, eviction/OOM counts, spot interruption rate, `ProdAdmin` assumption (§8.6); `ER_PROFILE_*` as span events.
- [ ] **4.4 Audit artifacts with teeth** (§8.6)
  - A full Aurora **restore test performed and its record written** — a backup never restored is a finding.
  - GuardDuty on in prod; break-glass procedure documented and walked through once.
  - The point-in-time SOC 2 set: policies, risk assessment, subprocessor inventory (Anthropic + the three IdPs' DPAs), vendor assessments.
  - The §16.3 monthly cost queries run once for real: per-tenant cost, cost-per-million-records, failed-run spend.

**Plan exit:** 4.1's zero-keys check passes, 4.4's restore record exists, and every §17
behavioural check — Phases 0 through 3 — has been re-run green within the last month.

---

## Standing rules while executing

1. **`make check-all` and the integration tier gate every merge** that touches images,
   compose, or the S3 config sites — the fast tiers pass while integration fails
   (established project history; see memory/CI notes).
2. **Measurements before scaling decisions**: §18.2 (thread knee) before widening the class
   ladder; §18.16 (pull time) before image surgery; §18.4 (telemetry volume) before
   widening the CloudTrail scope.
3. **When a task fails its acceptance criteria**, the fix goes through the design doc
   first if the design was wrong — this file never silently diverges from
   infrastructure.md.
