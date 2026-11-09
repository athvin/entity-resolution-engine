//! The job-dispatcher loop, shared by the standalone `er-dispatcher` binary and
//! `er-server`'s in-process mode (the merged `web-api` service). It polls the
//! Postgres SKIP-LOCKED queue; for each claimed job it opens the org's DuckLake
//! and runs the matching `er-cli` op IN-PROCESS (no per-run k8s Jobs), marks the
//! outcome, and emits the `job.<state>` webhook. Idle passes back off; a
//! Postgres failure is returned (fatal to the caller); `term` requests a drain
//! of the in-flight job then a clean exit.
//!
//! INPUT RESOLUTION (from the org row + env): the lake from `ER_LAKE_CATALOG` +
//! `ER_LAKE_DATA_PATH`; the config from `ER_CONFIG_PATH`; deliveries from
//! `<drop_root>/<source>.csv` per configured source; the model from
//! `ER_MODEL_PATH` + `ER_TF_PATH` (+ `ER_MODEL_VERSION` / `ER_TF_SNAPSHOT_ID`),
//! with an optional `ER_LEXICON_PATH`. A job whose required inputs are absent is
//! FAILED with a clear reason, never silently skipped.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::{Job, pg, webhook};

const IDLE_MIN: Duration = Duration::from_millis(500);
const IDLE_MAX: Duration = Duration::from_secs(5);

/// How often the scheduler tick runs. Cron's resolution is one minute and the
/// tick is idempotent per minute, so this only has to be comfortably under a
/// minute — it is not the firing precision.
const SCHEDULE_TICK_EVERY: Duration = Duration::from_secs(20);

/// How many webhook deliveries one loop pass may attempt.
///
/// Bounded so a backlog cannot starve job dispatch — whatever is left is still
/// due next pass. Small because each attempt is a network round trip with a
/// 10s ceiling; a bigger budget would let one unreachable subscriber stall the
/// loop for minutes.
const DELIVERY_BUDGET: usize = 8;

/// How often the S3 auto-ingest poller sweeps (E9c).
///
/// Slower than the scheduler tick on purpose. The sweep costs one listing per
/// configured source per tenant with auto-ingest on, which is a billed S3
/// request, and the thing it is waiting for — a human or a nightly export
/// dropping a file — does not happen on a twenty-second cadence. A minute of
/// latency on an import nobody is watching is free; the requests are not.
const S3_POLL_EVERY: Duration = Duration::from_secs(60);

/// How often the retention sweep runs. Hourly rather than daily so a restart
/// never postpones it a full day (each tick's deadline starts at boot), and
/// because an in-window sweep is one indexed probe per table — cheap enough
/// that cadence is not worth tuning.
const RETENTION_SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// Poll + run jobs until `term` is set or a Postgres failure occurs. Returns
/// `Ok(())` on a clean (term-requested) exit; `Err` on a control-plane failure
/// (the caller decides whether that's fatal — the standalone binary exits
/// non-zero so k8s restarts it; the in-process spawn logs and lets the API keep
/// serving).
///
/// # Errors
/// Propagates a Postgres claim/mark/lookup failure.
pub async fn run_loop(
    client: &tokio_postgres::Client,
    http: &reqwest::Client,
    term: &AtomicBool,
    mut executor: Option<crate::executor::Executor>,
    mut watch_frames: Option<tokio::sync::mpsc::UnboundedReceiver<er_k8s::WatchFrame>>,
) -> Result<(), tokio_postgres::Error> {
    if let Some(ex) = executor.as_mut() {
        ex.adopt().await;
    }
    tracing::info!("the dispatcher is up");
    let mut idle = IDLE_MIN;
    let mut next_schedule_tick = tokio::time::Instant::now();
    let mut next_s3_poll = tokio::time::Instant::now();
    let mut next_retention_sweep = tokio::time::Instant::now();
    // Built lazily: see the poll tick below.
    let mut s3: Option<crate::s3_ingest::Watcher> = None;
    while !term.load(Ordering::SeqCst) {
        // Schedules fire from here. Cheap (one query when nothing is due) and
        // rate-limited, so it rides the job loop rather than owning a task: one
        // loop means one place that stops on SIGTERM.
        if tokio::time::Instant::now() >= next_schedule_tick {
            next_schedule_tick = tokio::time::Instant::now() + SCHEDULE_TICK_EVERY;
            match schedule_tick(client, chrono::Utc::now()).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "the scheduler enqueued jobs"),
                // A tick failure must not take the job loop down with it: the
                // queue keeps draining and the next tick retries.
                Err(e) => tracing::warn!(error = %e, "a scheduler tick failed"),
            }
        }
        // Auto-ingest (E9c) rides the loop for the reason the scheduler tick
        // does: one loop means one thing to stop on SIGTERM. The client is
        // built on the first tick that finds a tenant with a bucket, so a
        // deployment where nobody has turned auto-ingest on — which is every
        // deployment until someone does — never constructs an AWS client and
        // never resolves a credential.
        if tokio::time::Instant::now() >= next_s3_poll {
            next_s3_poll = tokio::time::Instant::now() + S3_POLL_EVERY;
            match s3_poll_tick(client, &mut s3).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "s3 auto-ingest landed objects"),
                // Same rule as the scheduler tick and the webhook drain: a
                // failure here must not take the job loop down, or one
                // tenant's misconfigured bucket stops every tenant's jobs.
                Err(e) => tracing::warn!(error = %e, "an s3 auto-ingest tick failed"),
            }
        }

        // Retention (spec 5.24): retire audit and settled webhook-delivery
        // rows past the deployment window. Hourly and batched — cheap when
        // nothing is old enough, convergent over a backlog — and it rides the
        // loop for the same one-thing-to-stop-on-SIGTERM reason the others do.
        if tokio::time::Instant::now() >= next_retention_sweep {
            next_retention_sweep = tokio::time::Instant::now() + RETENTION_SWEEP_EVERY;
            match pg::purge_expired(client).await {
                Ok((0, 0)) => {}
                Ok((audit, deliveries)) => tracing::info!(
                    audit,
                    deliveries,
                    "the retention sweep retired expired rows"
                ),
                // The same rule as every tick: a failure must not take the
                // job loop down; the next sweep retries.
                Err(e) => tracing::warn!(error = %e, "a retention sweep failed"),
            }
        }

        // Deliver what the emit sites recorded. The outbox is drained here
        // rather than at emit time so a subscriber that is briefly unreachable
        // gets the event late instead of never — before this, every emit site
        // POSTed inline and dropped the outcome with `let _ =`.
        let drained = match webhook::drain(client, http, DELIVERY_BUDGET).await {
            Ok(n) => n,
            // A delivery failure is recorded, not propagated; a Postgres
            // failure here must not take the job loop down either, for the
            // same reason the scheduler tick does not.
            Err(e) => {
                tracing::warn!(error = %e, "the webhook drain failed");
                0
            }
        };

        // The executor's machinery (§5.1): watch transitions as they arrived,
        // then the passes — cancel sweep, readiness, launcher, the two sweeps.
        let mut executor_worked = false;
        if let Some(ex) = executor.as_mut() {
            if let Some(frames) = watch_frames.as_mut() {
                while let Ok(frame) = frames.try_recv() {
                    ex.apply_frame(frame).await;
                    executor_worked = true;
                }
            }
            executor_worked |= ex.passes().await;
        }

        if dispatch_once(client, executor.as_mut()).await? || drained > 0 || executor_worked {
            idle = IDLE_MIN; // did work; poll again immediately
        } else {
            tokio::time::sleep(idle).await; // empty queue; back off (SIGTERM
            idle = (idle * 2).min(IDLE_MAX); // is seen at the next loop top)
        }
    }
    tracing::info!("the dispatcher is draining and will exit");
    Ok(())
}

/// The actor every auto-ingested delivery is attributed to.
///
/// Named for the mechanism rather than borrowed from a person or an operator
/// key, the same way W0.2 stamps `created_by = 'scheduler'`: nobody authorised
/// this particular import, and attributing it to whoever last configured the
/// bucket would make the audit log read as though they had.
const S3_ACTOR: &str = "s3-ingest";

/// Sweep every tenant with auto-ingest configured, landing at most one object
/// per source (E9c). Returns how many landed.
///
/// # The order of the last two steps is the whole correctness argument
///
/// The delivery is published and *then* the cursor advances. The other order
/// is tempting — it would make a re-ingest impossible — and it is wrong: a
/// cursor advanced before a download that then fails would skip that object
/// permanently, and the tenant's file would be silently never read. This order
/// can at worst land the same bytes twice, which shows up as a duplicate run
/// in the job list and is recoverable. One failure mode is visible and
/// survivable; the other is silent data loss.
///
/// # Errors
/// Propagates a Postgres read failure. Per-tenant and per-source problems are
/// logged and stepped over — one tenant's misconfigured bucket must not stop
/// another tenant's imports.
pub async fn s3_poll_tick(
    client: &tokio_postgres::Client,
    watcher: &mut Option<crate::s3_ingest::Watcher>,
) -> Result<usize, tokio_postgres::Error> {
    let orgs = er_entities::org::read::with_s3_ingest(client).await?;
    if orgs.is_empty() {
        return Ok(0);
    }
    let watcher = match watcher {
        Some(w) => &*w,
        slot @ None => slot.insert(crate::s3_ingest::Watcher::from_env().await),
    };

    let mut landed = 0usize;
    for org in orgs {
        let Some(bucket) = org.s3_ingest_bucket.clone() else {
            continue;
        };
        // No prefix means the bucket root, which is a legitimate setup.
        let prefix = org.s3_ingest_prefix.clone().unwrap_or_default();
        let org_name = org.name.clone();
        let org_id = org.id.clone();
        // Cursors land in the org's one lake — the default dataset's, until
        // auto-ingest goes per-dataset.
        let org_dataset = pg::default_dataset_id(client, &org_id).await?;

        // One read for every source's cursor, rather than a query per source.
        let cursors: std::collections::HashMap<String, (String, Option<String>)> =
            er_entities::s3_ingest_cursor::read::by_org(
                client,
                org_dataset.as_deref().unwrap_or(&org_id),
            )
            .await?
            .into_iter()
            .map(|c| (c.source, (c.object_key, c.etag)))
            .collect();

        // The sources to sweep come from the ACTIVE config, not from listing
        // the bucket's directories: a tenant who drops a folder nobody
        // configured has not declared what its columns mean, and guessing
        // would attribute their records to a source that does not exist.
        let detail = pg::OrgDetail::from(org);
        let Ok(config) = crate::org_active_config(client, &detail).await else {
            tracing::warn!(
                er.org = %org_name,
                "s3 auto-ingest skipped the org: no readable active config to say which sources its prefix holds"
            );
            continue;
        };

        for source in config.sources.keys() {
            let mut skipped = Vec::new();
            let newest = watcher.newest(&bucket, &prefix, source, &mut skipped).await;
            for why in &skipped {
                tracing::info!(er.org = %org_name, er.source = %source, reason = ?why, "s3 auto-ingest passed over an object");
            }
            let found = match newest {
                Ok(Some(found)) => found,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(
                        er.org = %org_name,
                        er.source = %source,
                        error = %crate::errfmt::chain(&e),
                        "s3 auto-ingest failed for this source"
                    );
                    continue;
                }
            };
            let (drop, etag) = found;
            let seen = cursors
                .get(source)
                .map(|(key, tag)| (key.as_str(), tag.as_deref()));
            if !crate::s3_ingest::is_new(&drop, etag.as_deref(), seen) {
                continue;
            }

            // The same gate both hand-driven delivery paths go through, so an
            // auto-ingest cannot land on a suspended tenant that the API would
            // have refused. This is the check whose refusal needed a reason
            // rather than a status code, because this log line is its only
            // reader.
            // The poller is org-driven; its deliveries land in the org's one
            // converged dataset, by the name the sweep guarantees.
            let (_, _, _, drop_root) =
                match crate::delivery_preconditions(client, &org_name, "default", source).await {
                    Ok(t) => t,
                    Err(refusal) => {
                        tracing::warn!(
                            er.org = %org_name,
                            er.source = %source,
                            er.object_key = %drop.key,
                            reason = %refusal.reason(),
                            "s3 auto-ingest did not land an object"
                        );
                        continue;
                    }
                };

            let delivery_id = er_ids::new_v7();
            let Some(part) =
                crate::s3_ingest::staging_path(Path::new(&drop_root), source, &delivery_id)
            else {
                tracing::warn!(
                    er.org = %org_name,
                    er.source = %source,
                    "s3 auto-ingest refused the source: it would not stay inside the drop root"
                );
                continue;
            };
            let bytes = match watcher.download(&bucket, &drop.key, &part).await {
                Ok(n) => n,
                Err(e) => {
                    // A `.part` left behind would accumulate every tick.
                    let _ = tokio::fs::remove_file(&part).await;
                    tracing::warn!(
                        er.org = %org_name,
                        er.source = %source,
                        error = %crate::errfmt::chain(&e),
                        "s3 auto-ingest failed for this source"
                    );
                    continue;
                }
            };

            let response = crate::finish_delivery(
                client,
                S3_ACTOR,
                // The S3 poll tick is not a request: there is no click behind
                // this delivery, so the run roots its own trace rather than
                // claiming a parent it does not have.
                None,
                &crate::StagedDelivery {
                    org: &org_name,
                    org_id: &org_id,
                    source,
                    delivery_id: &delivery_id,
                    dataset_id: org_dataset.as_deref(),
                    temp_path: &part.to_string_lossy(),
                    bytes,
                    action: "import.s3",
                },
            )
            .await;
            if !response.status().is_success() {
                // The commonest case is the one-active-job-per-org index: a run
                // is already in flight. Leaving the cursor alone is the whole
                // retry mechanism — the next tick finds the same object still
                // new and lands it once the queue is free.
                tracing::warn!(
                    er.org = %org_name,
                    er.source = %source,
                    http.response.status_code = %response.status(),
                    "s3 auto-ingest could not publish; the cursor stays put so the next tick retries"
                );
                continue;
            }

            if let Err(e) = er_entities::s3_ingest_cursor::create::advance(
                client,
                &er_ids::new_v7(),
                &org_id,
                org_dataset.as_deref(),
                source,
                &drop.key,
                etag.as_deref(),
                drop.size,
                &delivery_id,
            )
            .await
            {
                // The delivery is already published and its run queued, so this
                // is not a failure of the import — it is a failure to remember
                // it, and the next tick will land the same object again. Loud,
                // because a cursor that never advances is an ingest loop.
                tracing::warn!(
                    er.org = %org_name,
                    er.source = %source,
                    error = %e,
                    "s3 auto-ingest landed but its cursor did not advance, so it will be re-ingested"
                );
                continue;
            }
            landed += 1;
        }
    }
    Ok(landed)
}

/// Enqueue a job for every enabled schedule whose cron matches `now`'s minute.
/// Returns how many were enqueued.
///
/// Before this existed, `schedules` was inert storage: the table, its CRUD and
/// its cron validation all shipped, but nothing ever read a row to enqueue
/// anything and `last_enqueued_at` was never written. Every schedule in the
/// product — including `config.correction_pass.cadence` — simply never fired.
///
/// Two properties worth stating, because both are deliberate:
///
/// **Exactly once per minute, even with several dispatchers.** The firing is
/// keyed `sched:<schedule_id>:<yyyymmddhhmm>` and `pg::enqueue` upserts on the
/// idempotency key, so two replicas ticking the same minute produce one job and
/// both see it. `last_enqueued_at` is written for operators to read; it is not
/// what makes the firing unique.
///
/// **No catch-up.** A dispatcher down across a firing does not backfill it on
/// return, exactly like cron(8). Backfilling would convert an outage into a
/// burst of identical runs, which for a `run_all_full` is the most expensive
/// thing this system can do.
///
/// # Errors
/// Propagates a Postgres read/enqueue failure.
pub async fn schedule_tick(
    client: &tokio_postgres::Client,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<usize, tokio_postgres::Error> {
    let minute_key = now.format("%Y%m%d%H%M").to_string();
    let mut enqueued = 0usize;
    for schedule in pg::due_schedules(client).await? {
        // A row whose cron cannot be parsed is reported, not silently skipped —
        // the API rejects these on the way in, so one here is a data-level
        // surprise worth seeing.
        let Ok(expr) = crate::cron::Cron::parse(&schedule.cron) else {
            tracing::warn!(
                er.schedule_id = %schedule.id,
                cron = ?schedule.cron,
                "the schedule has an unrunnable cron; skipping it"
            );
            continue;
        };
        if !expr.matches_at(now) {
            continue;
        }
        let key = format!("sched:{}:{minute_key}", schedule.id);
        let (_job, inserted) = pg::enqueue_scheduled(
            client,
            &schedule.org_id,
            Some(&schedule.dataset_id),
            &schedule.kind,
            &key,
            Some(&schedule.params),
            &schedule.id,
            // A cron tick has no caller to inherit from, so the run roots its
            // own trace. The tick gets a span of its own once the subscriber
            // exists, and this becomes that span's context.
            None,
        )
        .await?;
        // A replayed firing (same minute, another tick or another replica) comes
        // back as the existing row and must not be counted or re-stamped.
        if inserted {
            pg::mark_schedule_enqueued(client, &schedule.id).await?;
            enqueued += 1;
        }
    }
    Ok(enqueued)
}

/// A tenant's active registry model, resolved before the blocking half runs.
/// Claim one job and PLAN it. `Ok(true)` claimed and planned one, `Ok(false)`
/// there was nothing to claim, `Err` a Postgres failure (fatal to the caller).
///
/// The work itself then rides the executor's passes and watch as one
/// Kubernetes Job per task attempt. This function runs no stage: the dispatcher
/// stopped executing pipeline work when the in-process arm was deleted (#645).
///
/// **Without an executor it claims nothing**, deliberately. A dispatcher with
/// no namespace and no in-cluster identity — which is every host, and so every
/// `make run` — has nothing to execute a job with, and claiming one would mark
/// a row `running` that nothing could ever advance. Returning `Ok(false)` lets
/// the loop idle instead, which is what the Phase 4 decision (#638) chose for
/// local development: `make run` and `scripts/frontend-dev/up.sh` keep
/// API-only work.
///
/// There is no per-kind allowlist to consult, and no second path to fall
/// through to. A kind the DAG registry cannot parse is refused terminally by
/// `plan_claimed` with that reason on the row — a better answer than the arm's
/// generic "dispatcher does not handle job kind", and now the only one.
///
/// Takes no mailer and no provisioning profile: the executor owns both, because
/// it owns the settle that sends the run report and the control task that
/// provisions.
///
/// # Errors
/// Propagates a Postgres claim/plan failure.
pub async fn dispatch_once(
    client: &tokio_postgres::Client,
    executor: Option<&mut crate::executor::Executor>,
) -> Result<bool, tokio_postgres::Error> {
    let Some(ex) = executor else {
        return Ok(false);
    };
    let Some(job) = pg::claim(client, pg::org_job_cap()).await? else {
        return Ok(false);
    };
    ex.plan_claimed(&job).await;
    Ok(true)
}

/// How many approved reviews one tick will stage.
///
/// A cap rather than unbounded: a tenant that tags a thousand groups should see
/// them merged over several ticks instead of one job holding the org's single
/// writer slot for an hour. The remainder keeps its tag and is picked up next
/// tick, which is why removing the tag as it is staged matters.
pub(crate) const MAX_APPROVED: i64 = 200;

/// Job kinds a run report is sent for: the ones whose outcome a steward is
/// waiting on.
///
/// `lake_maintain` is deliberately absent — it is platform housekeeping on a
/// schedule, and a tenant cannot act on it, so mailing them would train them to
/// filter the whole thread. `provision` is absent because the tenant-create UI
/// already polls it with a progress indicator. Both still emit webhooks and
/// still appear in the runs list; this is about what is worth an interruption.
const REPORTED_KINDS: [&str; 4] = ["run_all_full", "run_all_incremental", "correct", "train"];

/// The org env key holding the report recipients, comma-separated.
const REPORT_TO_KEY: &str = "ER_RUN_REPORT_TO";

/// The run-report messages to send for a settled job, one per recipient.
///
/// Empty means send nothing, and there are four reasons for that: the tenant has
/// not configured reporting, the kind is not one a steward waits on, the job was
/// requeued rather than settled, or every configured address was blank.
///
/// **Recipients are an explicit address list, not a derived one.** er-server has
/// no user roster — membership lives in the frontend's own database, and
/// inventing a cross-store read for a notification would be a worse trade than
/// asking an operator for addresses. So `ER_RUN_REPORT_TO` sits in the org's
/// `env` beside the nine other `ER_*` keys the dispatcher already reads, and its
/// ABSENCE is the opt-out: a tenant who has not asked for mail does not get any.
///
/// Pure so the whole decision is testable without a mailer, a Postgres or a
/// job: every branch below is a rule someone will want to change later.
pub(crate) fn run_report_messages(
    org_name: &str,
    env: &Value,
    job: &Job,
    entity_counts: &[(String, i64)],
) -> Vec<crate::mail::Message> {
    // A requeued job has not settled; reporting per attempt is how three
    // notifications get sent for one incident. Same rule the webhook follows.
    if job.state == "queued" || !REPORTED_KINDS.contains(&job.kind.as_str()) {
        return Vec::new();
    }
    let Some(to) = env_str(env, REPORT_TO_KEY) else {
        return Vec::new();
    };
    let recipients: Vec<&str> = to
        .split(',')
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .collect();
    if recipients.is_empty() {
        return Vec::new();
    }

    let summary = entity_counts
        .iter()
        .map(|(kind, count)| format!("  {count} {kind}"))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    let mut params = std::collections::BTreeMap::new();
    params.insert("org".to_string(), json!(org_name));
    params.insert("kind".to_string(), json!(job.kind));
    params.insert("state".to_string(), json!(job.state));
    params.insert("job_id".to_string(), json!(job.id));
    if let Some(run_id) = &job.run_id {
        params.insert("run_id".to_string(), json!(run_id));
    }
    if let Some(detail) = &job.error_detail {
        params.insert("error".to_string(), json!(detail));
    }
    if !summary.is_empty() {
        params.insert("summary".to_string(), json!(summary));
    }

    recipients
        .into_iter()
        .map(|address| crate::mail::Message {
            to: address.to_string(),
            template: "run_report".to_string(),
            params: params.clone(),
            org: Some(org_name.to_string()),
        })
        .collect()
}

/// The `entity.*` webhook event name for one `entity_events.event_type`, or
/// `None` for a ledger kind that is not subscribable.
///
/// Spelled out as literals rather than `format!("entity.{kind}")` for two
/// reasons. The catalog gate reads the emit sites looking for the event string,
/// so a computed name would — correctly — fail it: a name nothing *says* is a
/// name nothing can be shown to emit. And it makes the exclusion explicit.
/// `edge_cut` is in the ledger's kind list but has no writer (the `cut_edges`
/// relation has no producer either), so it stays out of the catalog by the
/// catalog's own rule, rather than being listed and never delivered.
fn entity_event_name(kind: &str) -> Option<&'static str> {
    match kind {
        "created" => Some("entity.created"),
        "member_added" => Some("entity.member_added"),
        "member_removed" => Some("entity.member_removed"),
        "merged" => Some("entity.merged"),
        "split" => Some("entity.split"),
        "retired" => Some("entity.retired"),
        _ => None,
    }
}

/// Record one `entity.<kind>` webhook event per kind the run produced.
///
/// Aggregated per kind on purpose. O3a left entity events out of the catalog
/// precisely because "thousands of un-retried POSTs per run would be worse than
/// none", and the outbox O3b built fixes the retry half without touching the
/// volume: a 10M-row corpus still produces millions of ledger rows. So this is
/// at most six events per run, each carrying its count and where to read the
/// rows — the standard webhook shape of notify rather than deliver, and lossless
/// because the detail stays retrievable.
///
/// Best-effort, like the job event: the run's work is already committed, and a
/// subscriber lookup that fails must not retroactively fail the run. A kind with
/// no catalog entry is LOGGED rather than dropped silently, so a ledger kind
/// that gains a writer surfaces as an operator-visible line instead of as
/// events that quietly never arrive.
pub(crate) async fn emit_entity_aggregates(
    client: &tokio_postgres::Client,
    job: &Job,
    counts: &[(String, i64)],
) {
    let org_name = match pg::get_org_by_id(client, &job.org_id).await {
        Ok(Some(org)) => org.name,
        // Subscribers key on the org's id, so an unresolvable public handle
        // degrades the payload rather than skipping the delivery.
        Ok(None) => job.org_id.clone(),
        Err(e) => {
            tracing::warn!(
                er.job_id = %job.id,
                error = %e,
                "the entity-event aggregate was not recorded: resolving the org failed"
            );
            return;
        }
    };
    // The detail route lives under the job's dataset; a run without one (an
    // org-level job cannot produce entity events, but degrade rather than
    // skip) falls back to the sweep's universal name.
    let ds_name = match job.dataset_id.as_deref() {
        Some(id) => match pg::get_dataset_by_id(client, id).await {
            Ok(Some(d)) => d.name,
            _ => "default".to_string(),
        },
        None => "default".to_string(),
    };
    for (kind, n) in counts {
        let Some(event) = entity_event_name(kind) else {
            tracing::warn!(
                er.job_id = %job.id,
                count = n,
                er.event_kind = ?kind,
                "entity events have no catalog entry, so nothing could have subscribed to them; not delivered"
            );
            continue;
        };
        let data = json!({
            // The run id IS the job id (`run_job` takes `&job.id`), named both
            // ways because a subscriber correlating with `job.*` wants the one
            // and a subscriber fetching the detail wants the other.
            "run_id": job.id,
            "job_id": job.id,
            "event_type": kind,
            "count": n,
            "detail_path": format!(
                "/v1/orgs/{org_name}/datasets/{ds_name}/runs/{}/entity-events?event_type={kind}",
                job.id
            ),
        });
        if let Err(e) = webhook::emit(
            client,
            &job.org_id,
            job.dataset_id.as_deref(),
            &org_name,
            event,
            &data,
        )
        .await
        {
            tracing::warn!(er.job_id = %job.id, er.event = %event, error = %e, "the event was not recorded");
        }
    }
}

/// Seconds since the Unix epoch (the run-stamp wall clock).
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `secs` since the epoch as `YYYY-MM-DD HH:MM:SS` (UTC) — the lake's timestamp
/// literal form. Howard Hinnant's civil-from-days: exact + dependency-free.
pub(crate) fn fmt_epoch(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
}

fn env_str<'a>(env: &'a Value, key: &str) -> Option<&'a str> {
    env.get(key).and_then(Value::as_str)
}

/// Append a trained TF snapshot's rows to the shared frozen-TF CSV, writing the
/// header only when the file is new.
///
/// The file is keyed by `(model_version, tf_snapshot_id, column_name)`, so
/// snapshots coexist and each model reads only its own rows.
pub(crate) fn append_tf_snapshot(tf_path: &str, tf_rows: &str) -> Result<(), String> {
    use std::io::Write;
    let existing = std::fs::metadata(tf_path).is_ok_and(|m| m.len() > 0);
    let mut lines = tf_rows.lines();
    let header = lines.next().unwrap_or_default();
    let body: Vec<&str> = lines.filter(|l| !l.trim().is_empty()).collect();
    if body.is_empty() && existing {
        // A model with no TF columns trains fine and writes only a header;
        // appending nothing is the correct no-op, not a failure.
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(tf_path)
        .map_err(|e| format!("open {tf_path} to append the TF snapshot: {e}"))?;
    if !existing {
        writeln!(file, "{header}").map_err(|e| format!("write TF header: {e}"))?;
    }
    for line in body {
        writeln!(file, "{line}").map_err(|e| format!("append TF row: {e}"))?;
    }
    Ok(())
}

/// `CREATE DATABASE <db>` on the maintenance connection, if absent.
///
/// The identifier is built by [`crate::provision::tenant_database`], which folds
/// everything but `[a-z0-9]` to `_`, so it cannot carry a quote — but it is still
/// double-quoted here, because `CREATE DATABASE` takes no parameters and a
/// hand-built identifier is exactly the shape that becomes an injection the day
/// someone relaxes the folding.
pub(crate) async fn create_tenant_database(maint_dsn: &str, database: &str) -> Result<(), String> {
    let client = pg::connect(maint_dsn).await.map_err(|e| {
        format!(
            "connect to the provisioning maintenance DSN: {}",
            crate::errfmt::chain(&e)
        )
    })?;
    let existing = client
        .query("SELECT 1 FROM pg_database WHERE datname = $1", &[&database])
        .await
        .map_err(|e| format!("look up database {database}: {e}"))?;
    if !existing.is_empty() {
        return Ok(());
    }
    let quoted = database.replace('"', "\"\"");
    client
        .batch_execute(&format!("CREATE DATABASE \"{quoted}\""))
        .await
        .map_err(|e| format!("create database {database}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fmt_epoch;

    #[test]
    fn fmt_epoch_matches_known_instants() {
        assert_eq!(fmt_epoch(0), "1970-01-01 00:00:00");
        assert_eq!(fmt_epoch(1_767_323_045), "2026-01-02 03:04:05");
    }
}

#[cfg(test)]
mod run_report_tests {
    use super::{REPORT_TO_KEY, run_report_messages};
    use crate::Job;
    use serde_json::json;

    fn job(kind: &str, state: &str) -> Job {
        let at = "2026-01-02T03:04:05Z".parse().expect("a fixed timestamp");
        Job {
            id: "01JOB000000000000000000001".to_string(),
            org_id: "uuid-acme".to_string(),
            dataset_id: None,
            kind: kind.to_string(),
            params: json!({}),
            state: state.to_string(),
            priority: 0,
            idempotency_key: None,
            run_id: Some("01RUN000000000000000000001".to_string()),
            attempt: 1,
            max_attempts: 3,
            error_class: None,
            error_detail: None,
            not_before: None,
            created_by: None,
            schedule_id: None,
            resource_class: None,
            trace_context: None,
            structural_fingerprint: None,
            policy_hash: None,
            image_digest: None,
            clear_number: None,
            owner_epoch: None,
            created_at: at,
            updated_at: at,
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn a_tenant_that_asked_for_reports_gets_one_per_recipient() {
        let env = json!({ REPORT_TO_KEY: "ops@acme.test, steward@acme.test" });
        let sent = run_report_messages("acme", &env, &job("run_all_full", "succeeded"), &[]);
        assert_eq!(
            sent.iter().map(|m| m.to.as_str()).collect::<Vec<_>>(),
            vec!["ops@acme.test", "steward@acme.test"],
            "addresses are split on commas and trimmed"
        );
        assert!(sent.iter().all(|m| m.template == "run_report"));
        assert_eq!(sent[0].org.as_deref(), Some("acme"));
    }

    #[test]
    fn absence_is_the_opt_out() {
        // The default must be silence. A tenant who never asked for mail must
        // not receive any because a run happened to finish.
        assert!(
            run_report_messages("acme", &json!({}), &job("run_all_full", "succeeded"), &[])
                .is_empty()
        );
        // Configured-but-blank is the same answer, not a message to "".
        for blank in ["", "   ", ",", " , "] {
            let env = json!({ REPORT_TO_KEY: blank });
            assert!(
                run_report_messages("acme", &env, &job("run_all_full", "succeeded"), &[])
                    .is_empty(),
                "blank recipients {blank:?} must send nothing rather than mail an empty address"
            );
        }
    }

    #[test]
    fn a_requeued_job_is_not_an_outcome() {
        // The rule the webhook already follows: a job that fails twice and then
        // succeeds is ONE outcome. Reporting per attempt is how three
        // notifications get sent for one incident.
        let env = json!({ REPORT_TO_KEY: "ops@acme.test" });
        assert!(run_report_messages("acme", &env, &job("run_all_full", "queued"), &[]).is_empty());
    }

    #[test]
    fn housekeeping_kinds_are_not_reported() {
        let env = json!({ REPORT_TO_KEY: "ops@acme.test" });
        for kind in ["lake_maintain", "provision", "apply_corrections"] {
            assert!(
                run_report_messages("acme", &env, &job(kind, "succeeded"), &[]).is_empty(),
                "{kind} is not a kind a steward waits on, and mailing it teaches them to filter \
                 the whole thread"
            );
        }
        for kind in ["run_all_full", "run_all_incremental", "correct", "train"] {
            assert_eq!(
                run_report_messages("acme", &env, &job(kind, "succeeded"), &[]).len(),
                1,
                "{kind} changes a tenant's corpus or produces a model, so it is reported"
            );
        }
    }

    #[test]
    fn a_failure_carries_its_reason_and_a_success_carries_its_counts() {
        let env = json!({ REPORT_TO_KEY: "ops@acme.test" });

        let mut failed = job("correct", "failed");
        failed.error_detail = Some("no model_registry row has status='active'".to_string());
        let sent = run_report_messages("acme", &env, &failed, &[]);
        let rendered = crate::mail::render(&sent[0]).expect("renders");
        assert!(
            rendered.subject.contains("correct failed"),
            "{}",
            rendered.subject
        );
        assert!(
            rendered.body.contains("no model_registry row"),
            "a failure report must say why: {}",
            rendered.body
        );

        let counts = vec![("merged".to_string(), 4_i64), ("created".to_string(), 2)];
        let ok = run_report_messages("acme", &env, &job("run_all_full", "succeeded"), &counts);
        let rendered = crate::mail::render(&ok[0]).expect("renders");
        assert!(
            rendered.body.contains("4 merged") && rendered.body.contains("2 created"),
            "a success report must say what changed: {}",
            rendered.body
        );
        assert!(
            !rendered.body.contains("Why it failed"),
            "a success must not carry a failure section: {}",
            rendered.body
        );
    }
}
