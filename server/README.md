# erserver — the control plane

The multi-tenant SaaS backend over the `er` engine, implementing
[docs/backend-design.md](../docs/backend-design.md) through its Phase 1–3
scope. A **standalone uv project**, deliberately not a workspace member: the
engine's dependency set is spec-locked (DesignDoc S2.1) and its
`pyproject.toml`/`uv.lock` are contract-tested and copied verbatim into the
pipeline image build, so nothing at the repo root references this directory.
The server consumes `er` as an editable path dependency.

## Deployables

| Module | Role |
|---|---|
| `erserver.api` | FastAPI control plane. Never executes pipeline stages. |
| `erserver.dispatcher` | Leader loop: claims jobs, launches runners, ticks cron schedules, drains staged steward actions, delivers webhooks, reaps stale jobs at startup. |
| `erserver.runner` | One subprocess per pipeline run, executing through `er.service` under the org's injected `ER_*` environment. |

Run locally (single-VM mode; the k8s-Job launcher is a later substitution
behind the same `launch` seam):

```sh
export ERSERVER_DSN=postgresql://…       # control-plane Postgres
export ERSERVER_OPERATOR_TOKEN=…         # platform operator bearer token
uv run --project server uvicorn --factory erserver.api:create_app
uv run --project server python -m erserver.dispatcher
```

Automatic tenant provisioning (`POST /v1/orgs` without `config_path`)
additionally needs:

```sh
export ERSERVER_MAINT_DSN=postgresql://…            # role with CREATEDB (NOT superuser
                                                    # in prod — a dedicated er_provisioner);
                                                    # read by the runner at provision time
export ERSERVER_TENANT_DSN_TEMPLATE='postgresql://…/{dbname}'  # each org's ER_CATALOG_DSN
export ERSERVER_CONFIG_ROOT=/var/lib/er/configs     # server-managed {org}.yaml files
export ERSERVER_DROP_ROOT=/var/lib/er/drop          # per-org drop dirs
# optional:
export ERSERVER_LAKE_DATA_PATH_TEMPLATE='s3://er-lake/{ns}/'   # the default
export ERSERVER_CONFIG_TEMPLATE=…                   # defaults to the repo's configs/default.yaml
export ERSERVER_TENANT_ENV_JSON='{"ER_S3_ENDPOINT":"…","ER_S3_SECRET_ACCESS_KEY":"secret://S3",…}'
                                                    # shared ER_* merged into every new org's env
```

## What exists

- **Jobs** — Postgres `SKIP LOCKED` queue; one active job per org by unique
  index (the engine is single-writer per tenant); retries keyed to the
  engine's S4.0 exit codes (`transient_io` resumes the run, `lock_conflict`
  requeues without consuming an attempt); Idempotency-Key submission;
  cancel (SIGTERM, resumable) and resume; live per-stage heartbeats streamed
  off the engine's S5.2 stderr records. Kinds: `run_all_full`,
  `run_all_incremental`, `correct`, `train`, `provision` (operator-only,
  never schedulable).
- **Tenant provisioning** — `POST /v1/orgs {"name": …}` (no `config_path`)
  onboards a tenant end to end: the server derives the namespace
  (`t_<org>_<hash>`), seeds config version 1 from `configs/default.yaml`,
  issues a one-time admin key, and enqueues a `provision` job that creates the
  org's **dedicated Postgres database** (`er_t_<org>_<hash>` — hard isolation:
  no tenant's tables share a database with another's) and runs `er init`
  inside it under the org's own S3 prefix. Orgs carry a lifecycle `state`
  (`provisioning → active`, with `suspended`/`purging` reserved); every job
  submission is refused with 409 until the provision job flips the org
  active. `GET /v1/orgs/{org}` is the poll target. Passing `config_path`
  keeps the old manual/BYO behavior: the operator provisioned out of band and
  the org starts `active`. The whole flow is idempotent — a replayed POST
  returns the same provision job and never re-issues the admin key.
- **Auth** — org-scoped API keys (`erk_…`, hashed at rest) with
  admin/steward/viewer roles, operator token for provisioning, audit log on
  every mutation.
- **Config service** — versioned YAML validated by the engine's own loader,
  publish classifies the change tier (A re-band / B match rebuild / C retrain),
  refuses operator-only blocks for tenant admins, writes the runner's config
  file, syncs the `correction_pass.cadence` cron into a schedule, and enqueues
  the rebuild the tier costs.
- **Schedules** — cron rows ticked by the dispatcher with fire-time
  idempotency keys; includes the engine-declared correction pass, executed at
  last.
- **Read path** — per-tenant DuckLake reads under the engine's env overlay
  (`er.lake.env.lake_environment`): golden records with snapshot-pinned keyset
  pagination (`AT (VERSION => n)`), entity detail (members, lineage, events),
  duplicate groups, runs, metrics, merge-plan export (JSON/CSV).
- **Steward writes** — review resolution and always/never assertions, tried
  inline under the tenant writer lock and staged in the control plane on
  conflict; the dispatcher drains staged actions when the org goes quiet;
  `apply_now` enqueues the reconcile that makes an assertion take effect.
- **Imports** — the drop-dir connector's push seam: multipart upload lands in
  the org's drop root and enqueues an incremental run.
- **Webhooks** — `job.completed` delivery with HMAC-SHA256 signatures,
  posted off the job path so a slow subscriber never delays the queue.
- **Secret references** — org env values written as `secret://NAME` resolve at
  use time from `ERSERVER_SECRET_NAME` in the server's environment; the
  control-plane database stores only the reference.
- **Concurrency** — `ERSERVER_CONCURRENCY` runner workers (default 2); the
  one-active-per-org constraint means workers only ever parallelize across
  orgs, never within one.
- **Read pooling** — per-tenant DuckLake attaches are pooled (small LRU);
  safe because DuckLake resolves the snapshot from the catalog at query time,
  so a warm attach still sees other writers' commits.

## The connector seam

A connector integrates by exactly four contracts (design §6/§10): (a) deliver
files into the org's drop root (or POST `/v1/orgs/{org}/imports`), (b) its
field mapping compiles into the config's `sources:` block, (c) it submits jobs
with idempotency keys, (d) it consumes `job.completed` webhooks. The Salesforce
connector (OAuth, Bulk API sync, polling change capture, `merge()` writeback)
is design §10 work that requires a Salesforce org and is not implemented here.

## Tests

```sh
# bare (no services): policy matrix, runner-through-facade, cron logic
uv run --project server --extra test pytest server/tests

# + control-plane Postgres: queue, auth, dispatcher, schedules, config, webhooks
docker run -d --rm --name erserver-test-pg -p 5433:5432 -e POSTGRES_PASSWORD=er postgres:16
ERSERVER_TEST_DSN=postgresql://postgres:er@localhost:5433/postgres \
  uv run --project server --extra test pytest server/tests

# + full lake substrate: the end-to-end story (provision → import → train →
# full run → incremental → steward split → publish → merge plans), ~2–3 min
docker run -d --rm --name er-e2e-catalog -p 5434:5432 \
  -e POSTGRES_PASSWORD=er -e POSTGRES_DB=ducklake postgres:16 \
  -c max_locks_per_transaction=1024
docker run -d --rm --name er-e2e-minio -p 9000:9000 \
  -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
  minio/minio server /data
# create the `lake` bucket, then:
ERSERVER_E2E=1 ERSERVER_TEST_DSN=postgresql://postgres:er@localhost:5433/postgres \
  uv run --project server --extra test pytest server/tests/test_lake_e2e.py

# auto-provisioning end to end (same substrate): one POST → dedicated
# database + er init + active org
ERSERVER_E2E=1 ERSERVER_TEST_DSN=postgresql://postgres:er@localhost:5433/postgres \
  uv run --project server --extra test pytest server/tests/test_provision_e2e.py
```

## Deliberately not built (design phases 4–5 / needs external systems)

Salesforce connector and merge writeback (no org to test against), the k8s-Job
launcher (same `launch` seam, no cluster here), OIDC human auth, per-tenant STS
credentials, the Postgres FTS search escape hatch, Marketo, custom entity
types. The engine remains person-centric (design §13).

One label to read correctly: config publish reports a cost **tier** (A/B/C),
but every tier currently enqueues a full rebuild — the engine's own drift
guard and version-bump invariants (S4.0/S5.1) escalate any config-hash change
to a full run, so a cheaper tier-A path would be undone at the next
incremental anyway. The tier is the honest cost estimate and audit label, and
the seam a cheaper path plugs into if the engine ever relaxes that.
