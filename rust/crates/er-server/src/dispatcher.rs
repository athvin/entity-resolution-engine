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
) -> Result<(), tokio_postgres::Error> {
    eprintln!("dispatcher up");
    let mut idle = IDLE_MIN;
    let mut next_schedule_tick = tokio::time::Instant::now();
    while !term.load(Ordering::SeqCst) {
        // Schedules fire from here. Cheap (one query when nothing is due) and
        // rate-limited, so it rides the job loop rather than owning a task: one
        // loop means one place that stops on SIGTERM.
        if tokio::time::Instant::now() >= next_schedule_tick {
            next_schedule_tick = tokio::time::Instant::now() + SCHEDULE_TICK_EVERY;
            match schedule_tick(client, chrono::Utc::now()).await {
                Ok(0) => {}
                Ok(n) => eprintln!("scheduler enqueued {n} job(s)"),
                // A tick failure must not take the job loop down with it: the
                // queue keeps draining and the next tick retries.
                Err(e) => eprintln!("scheduler tick failed: {e}"),
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
                eprintln!("webhook drain failed: {e}");
                0
            }
        };

        if dispatch_once(client).await? || drained > 0 {
            idle = IDLE_MIN; // did work; poll again immediately
        } else {
            tokio::time::sleep(idle).await; // empty queue; back off (SIGTERM
            idle = (idle * 2).min(IDLE_MAX); // is seen at the next loop top)
        }
    }
    eprintln!("dispatcher draining -> exit");
    Ok(())
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
            eprintln!(
                "schedule {} has an unrunnable cron {:?}; skipping",
                schedule.id, schedule.cron
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
            &schedule.kind,
            &key,
            Some(&schedule.params),
            &schedule.id,
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
/// Owned rather than borrowed because it crosses a `spawn_blocking` boundary.
struct ActiveModel {
    model_json: String,
    model_version_label: String,
    tf_snapshot_id: Option<String>,
    name: String,
    version: i64,
}

/// Claim + run one job. `Ok(true)` ran a job (outcome persisted), `Ok(false)`
/// the queue was empty, `Err` a Postgres failure (fatal to the caller).
///
/// Takes no HTTP client: the job's `job.<state>` event is only RECORDED here,
/// and `run_loop`'s drain does the sending. Public so the Postgres-tier tests
/// can drive one real claim-run-mark cycle without the loop.
///
/// # Errors
/// Propagates a Postgres claim/mark/lookup failure.
pub async fn dispatch_once(client: &tokio_postgres::Client) -> Result<bool, tokio_postgres::Error> {
    dispatch_once_with(
        client,
        &crate::provision::Profile::from_env(),
        &crate::mail::Mailer::from_env().await,
    )
    .await
}

/// [`dispatch_once`] with the provisioning profile handed in rather than read
/// from the process environment — the Postgres-tier tests cannot set env
/// (`std::env::set_var` is `unsafe` in edition 2024 and the workspace denies
/// `unsafe_code`), and the `provision` arm needs one.
///
/// # Errors
/// Propagates a Postgres claim/mark/lookup failure.
pub async fn dispatch_once_with(
    client: &tokio_postgres::Client,
    profile: &Result<Option<crate::provision::Profile>, crate::provision::ProfileError>,
    mailer: &crate::mail::Mailer,
) -> Result<bool, tokio_postgres::Error> {
    let Some(job) = pg::claim(client).await? else {
        return Ok(false);
    };
    pg::mark(client, &job.id, "running").await?;

    let Some(org) = pg::get_org_by_id(client, &job.org_id).await? else {
        pg::mark(client, &job.id, "failed").await?;
        eprintln!("job {} failed: org id '{}' not found", job.id, job.org_id);
        return Ok(true);
    };

    // Every lake job reconciles its lake first, under the one-active-job-per-org
    // rule it already holds: the catalog compiled into this binary against the
    // tenant's live tables, safe statements applied and ledgered, a breaking
    // difference failing the job before any stage runs against a table it does
    // not fit. The outcome is recorded per tenant whatever it was, so the deploy
    // report and the drift check read it without opening the lake. Provisioning
    // is the one kind that runs before the lake exists; it reconciles the lake
    // it creates.
    let schema = if job.kind == "provision" {
        None
    } else {
        let env = org.env.clone();
        let outcome = tokio::task::spawn_blocking(move || crate::schema::reconcile_lake_env(&env))
            .await
            .unwrap_or_else(|e| Err(format!("dispatcher task panicked: {e}")));
        let (status, hash, detail) = crate::schema::tenant_state(&outcome);
        crate::schema::record_tenant_state(client, &job.org_id, status, &hash, Some(&detail))
            .await?;
        if let Ok(r) = &outcome
            && !r.applied.is_empty()
        {
            eprintln!("job {}: lake schema migrated ({})", job.id, r.detail());
        }
        Some(outcome)
    };

    // The pipeline is synchronous and holds a !Send DuckDB Session, so run it on
    // a blocking thread with owned inputs. The steward apply path is special: it
    // drains the org's pending staged actions into the lake and corrects (reusing
    // the existing scores), which needs the async Postgres client, so it does not
    // go through the blocking `run_job`.
    let env = org.env.clone();
    // Per-kind entity-event counts, for the webhook aggregate (O3c-2). Only a
    // reconciling run produces any; every other kind leaves this empty, which
    // is the honest "nothing to announce".
    let mut entity_counts: Vec<(String, i64)> = Vec::new();
    let outcome = if let Some(Err(message)) = schema {
        Err(message)
    } else if job.kind == crate::schema::LAKE_MIGRATE_KIND {
        // The sweep's job: reconciling was the work.
        Ok(())
    } else if job.kind == "provision" {
        // Async-half too, and for a different reason from the others: its final
        // act is a STATE TRANSITION on the org row, which is what the UI's
        // progress panel is waiting for.
        provision_tenant(client, &org, &job.id, profile).await
    } else if job.kind == "apply_corrections" {
        drain_and_correct(client, &job.org_id, &env, &job.id).await
    } else if job.kind == "train" {
        // Also async-half, and for the same reason as `apply_corrections`:
        // training's OUTPUT is a registry row, which needs the Postgres client
        // that `run_job`'s blocking half cannot hold.
        train_and_register(client, &job.org_id, &env, &job.id).await
    } else {
        // The active model comes from the registry, read here because `run_job`
        // is the blocking half and cannot touch the async Postgres client. An
        // org with no registry version resolves `None` and `run_job` falls back
        // to the env file, which is what every org did before the registry
        // existed.
        // EVERY active version, not one: activation is unique per (org, name)
        // since E5b-1, so an org can have several active models and the
        // published config's `models:` list says which of them score. The
        // selection happens in `run_job`, which is the half that holds the
        // config; this half just does the Postgres read it exists to do.
        let models: Vec<ActiveModel> = match pg::active_models(client, &job.org_id).await {
            Ok(rows) => rows
                .into_iter()
                .map(|m| ActiveModel {
                    model_json: m.model_json,
                    model_version_label: m.model_version_label,
                    tf_snapshot_id: m.tf_snapshot_id,
                    name: m.name,
                    version: m.version,
                })
                .collect(),
            Err(e) => {
                pg::mark(client, &job.id, "failed").await?;
                eprintln!("job {} failed: reading the model registry: {e}", job.id);
                return Ok(true);
            }
        };
        let jb = job.clone();
        let drop_root = org.drop_root.clone();
        // The counts come out beside the outcome rather than inside it: every
        // other arm of this `if` yields `Result<(), _>`, and widening all of
        // them to carry a value only this one can produce would be noise.
        match tokio::task::spawn_blocking(move || run_job(&jb, &env, drop_root.as_deref(), &models))
            .await
            .unwrap_or_else(|e| Err(format!("dispatcher task panicked: {e}")))
        {
            Ok(counts) => {
                entity_counts = counts;
                Ok(())
            }
            Err(message) => Err(message),
        }
    };

    let completed = match &outcome {
        Ok(()) => {
            pg::mark(client, &job.id, "succeeded").await?;
            Job {
                state: "succeeded".to_string(),
                ..job
            }
        }
        Err(message) => {
            // Classify before deciding: only a transient failure earns a retry.
            // Retrying a precondition (no model yet) or a bad config would burn
            // attempts on something that cannot change without a human, and the
            // UI already tells the operator which of those it is.
            let class = pg::FailureClass::classify(message);
            let after = pg::fail_or_retry(client, &job.id, class, message).await?;
            eprintln!(
                "job {} ({}) failed [{}] -> {}{}: {message}",
                job.id,
                job.kind,
                class.as_str(),
                after.state,
                if after.state == "queued" {
                    format!(" (attempt {} of {})", after.attempt, after.max_attempts)
                } else {
                    String::new()
                }
            );
            after
        }
    };
    // One webhook per OUTCOME, not per attempt: a job that fails twice and then
    // succeeds is one success to a subscriber, and three notifications for one
    // incident is how an integration learns to ignore them. A requeued job is
    // not an outcome yet.
    if completed.state != "queued" {
        let _ = webhook::emit_job_event(client, &completed).await;
    }
    // The entity-event aggregate rides the same settle, after the outcome: a
    // subscriber watching both should see the outcome first, because `entity.*`
    // describes what a run that SUCCEEDED did to the corpus.
    if completed.state == "succeeded" && !entity_counts.is_empty() {
        emit_entity_aggregates(client, &completed, &entity_counts).await;
    }
    // The run report rides the same settle, last. Mail is best-effort by
    // design — every caller has a fallback — so a send that fails is logged and
    // dropped rather than failing a job that already succeeded. Reporting an
    // outcome is not part of producing it.
    for message in run_report_messages(&org.name, &org.env, &completed, &entity_counts) {
        if let Err(e) = mailer.send(&message).await {
            eprintln!(
                "run report for job {} to {} failed: {e}",
                completed.id, message.to
            );
        }
    }
    Ok(true)
}

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
fn run_report_messages(
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
async fn emit_entity_aggregates(
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
            eprintln!(
                "run {}: entity-event aggregate not recorded: resolving the org: {e}",
                job.id
            );
            return;
        }
    };
    for (kind, n) in counts {
        let Some(event) = entity_event_name(kind) else {
            eprintln!(
                "run {}: {n} {kind:?} entity events have no catalog entry, so nothing \
                 could have subscribed to them — not delivered",
                job.id
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
                "/v1/orgs/{org_name}/runs/{}/entity-events?event_type={kind}",
                job.id
            ),
        });
        if let Err(e) = webhook::emit(client, &job.org_id, &org_name, event, &data).await {
            eprintln!("run {}: {event} not recorded: {e}", job.id);
        }
    }
}

/// Seconds since the Unix epoch (the run-stamp wall clock).
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `secs` since the epoch as `YYYY-MM-DD HH:MM:SS` (UTC) — the lake's timestamp
/// literal form. Howard Hinnant's civil-from-days: exact + dependency-free.
fn fmt_epoch(secs: u64) -> String {
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

fn load_lexicon(path: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            l.split_once(',')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        })
        .collect()
}

/// Dispatch one job to its `er-cli` op over the org's lake. Synchronous (runs on
/// a blocking thread). Returns a human error string on any failure.
fn run_job(
    job: &Job,
    env: &Value,
    drop_root: Option<&str>,
    active_models: &[ActiveModel],
) -> Result<Vec<(String, i64)>, String> {
    let catalog = env_str(env, "ER_LAKE_CATALOG").ok_or("org env missing ER_LAKE_CATALOG")?;
    let data_path = env_str(env, "ER_LAKE_DATA_PATH").ok_or("org env missing ER_LAKE_DATA_PATH")?;
    let session = er_cli::open_lake(catalog, data_path).map_err(|e| format!("open lake: {e:#}"))?;
    let run_id = &job.id;
    let now = fmt_epoch(unix_now());

    // Config (needed by every kind except lake_maintain).
    let load_config = || -> Result<er_config::Config, String> {
        let path = env_str(env, "ER_CONFIG_PATH").ok_or("org env missing ER_CONFIG_PATH")?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("read config {path}: {e}"))?;
        er_config::load_config_str(&text)
            .map_err(|e| format!("config: {}", crate::errfmt::chain(&e)))
    };

    match job.kind.as_str() {
        // Reconciled before this function ran; nothing else to do.
        crate::schema::LAKE_MIGRATE_KIND => Ok(()),
        "lake_maintain" => {
            let expire = env_str(env, "ER_EXPIRE_OLDER_THAN").unwrap_or("7 days");
            er_cli::run_maintain(&session, expire)
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
        }
        "correct" => {
            let config = load_config()?;
            er_cli::run_correct(&session, &config, run_id, &now).map_err(|e| format!("{e:#}"))
        }
        "run_all_full" | "run_all_incremental" => {
            let config = load_config()?;
            // `{"skip_ingest": true}` — set by every publish-triggered rebuild —
            // means "re-run the pipeline over the corpus already landed". It was
            // written by six call sites and read by none, so a rebuild re-read the
            // drop directory and FAILED with "no source deliveries found" whenever
            // the deliveries had been cleaned up after landing.
            let skip_ingest = job
                .params
                .get("skip_ingest")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let deliveries: Vec<(String, String)> = if skip_ingest {
                Vec::new()
            } else {
                let drop_root = drop_root.ok_or("org has no drop_root for a run_all")?;
                let found: Vec<(String, String)> = config
                    .sources
                    .keys()
                    .filter_map(|source| {
                        std::fs::read_to_string(format!("{drop_root}/{source}.csv"))
                            .ok()
                            .map(|csv| (source.clone(), csv))
                    })
                    .collect();
                if found.is_empty() {
                    return Err(format!("no source deliveries found under {drop_root}"));
                }
                found
            };
            // The TF snapshot is still a file on disk: the registry stores the
            // settings document and the identity it stamps, not the frozen term
            // frequencies, so `ER_TF_PATH` stays the source for those.
            let tf_path = env_str(env, "ER_TF_PATH").ok_or("org env missing ER_TF_PATH")?;
            let lexicon = env_str(env, "ER_LEXICON_PATH")
                .map(load_lexicon)
                .unwrap_or_default();
            // Registry first, env file second. The fallback is what keeps an org
            // created before the registry existed scoring exactly as it did.
            // With no `models:` block the org scores against its ONE active
            // version, which is what every config written before ensembles
            // means. Several active names and no block is not a default we may
            // pick for the tenant — activation became per-name in E5b-1, so this
            // state is reachable, and silently choosing one model is the class of
            // bug this program keeps closing.
            let active_model = if config.models.is_empty() {
                if active_models.len() > 1 {
                    let names: Vec<&str> = active_models.iter().map(|m| m.name.as_str()).collect();
                    return Err(format!(
                        "the registry has {} active models ({}) but the published config \
                         declares no `models:` list, so which one scores is undefined; \
                         declare the ensemble or archive the versions you do not want",
                        active_models.len(),
                        names.join(", ")
                    ));
                }
                active_models.first()
            } else {
                None
            };
            let from_env;
            let settings_json: &str = if let Some(m) = active_model {
                eprintln!(
                    "run {run_id}: model {}/{} ({})",
                    m.name, m.version, m.model_version_label
                );
                &m.model_json
            } else {
                let model_path =
                    env_str(env, "ER_MODEL_PATH").ok_or("org env missing ER_MODEL_PATH")?;
                from_env =
                    std::fs::read_to_string(model_path).map_err(|e| format!("read model: {e}"))?;
                &from_env
            };
            let model = er_cli::MatchModel {
                settings_json,
                tf_csv_path: Path::new(tf_path),
                model_version: active_model.map_or_else(
                    || env_str(env, "ER_MODEL_VERSION").unwrap_or("v0001"),
                    |m| m.model_version_label.as_str(),
                ),
                tf_snapshot_id: active_model
                    .and_then(|m| m.tf_snapshot_id.as_deref())
                    .or_else(|| env_str(env, "ER_TF_SNAPSHOT_ID"))
                    .unwrap_or("tf"),
            };
            // The ensemble: the published config's `models:` list resolved
            // against the registry's active set, in CONFIG order — the config is
            // the selector, the registry supplies the versions. So retraining a
            // member is an activate, not a publish.
            let members: Vec<er_cli::EnsembleMember<'_>> = config
                .models
                .iter()
                .map(|spec| {
                    let found = active_models
                        .iter()
                        .find(|m| m.name == spec.name)
                        .ok_or_else(|| {
                            let active: Vec<&str> =
                                active_models.iter().map(|m| m.name.as_str()).collect();
                            format!(
                                "the config declares model '{}' but the registry has no ACTIVE \
                                 version of that name (active: {})",
                                spec.name,
                                if active.is_empty() {
                                    "none".to_string()
                                } else {
                                    active.join(", ")
                                }
                            )
                        })?;
                    Ok(er_cli::EnsembleMember {
                        model: er_cli::MatchModel {
                            settings_json: &found.model_json,
                            tf_csv_path: Path::new(tf_path),
                            model_version: &found.model_version_label,
                            tf_snapshot_id: found.tf_snapshot_id.as_deref().unwrap_or("tf"),
                        },
                        name: &found.name,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            if !members.is_empty() {
                let listed: Vec<String> = members
                    .iter()
                    .map(|m| format!("{}={}", m.name, m.model.model_version))
                    .collect();
                eprintln!("run {run_id}: ensemble {}", listed.join(" + "));
            }

            // The mode the job asked for, then the drift guard's verdict on it.
            // An incremental run may only extend a corpus scored under the same
            // config / model / std / survivorship / lexicon fingerprint; when any
            // of those moved, the run is PROMOTED to full (rather than refused —
            // a refusal here is just a failed job, while a promotion is the thing
            // the operator actually wants) and the ledger records why.
            let asked = if job.kind == "run_all_incremental" {
                er_versions::MODE_INCREMENTAL
            } else {
                er_versions::MODE_FULL
            };
            let prior = er_cli::prior_fingerprint(&session).map_err(|e| format!("{e:#}"))?;
            let current = er_cli::current_fingerprint(&config, model.model_version, &lexicon);
            let decision =
                er_versions::check_mode_preconditions(prior.as_ref(), &current, asked, true)
                    .map_err(|e| format!("mode guard: {e}"))?;
            if decision.escalated() {
                eprintln!("run {run_id}: {}", decision.message);
            }
            let rebuild_reason = if decision.escalated() {
                Some(er_versions::REBUILD_OPERATOR)
            } else {
                None
            };

            // `{"segment": "<name>"}` restricts the match pass to a slice of the
            // corpus (#54). The job carries the NAME; the predicate is looked up
            // in the org's own config, which was parsed and column-checked at
            // publish. A job carrying SQL would be an injection surface, and the
            // enqueueing endpoint validates the name too — this is the second
            // check, because a schedule stores its params and the config can be
            // republished without it.
            let segment_name = job.params.get("segment").and_then(Value::as_str);
            let segment = match segment_name {
                Some(name) => {
                    let predicate = config.segments.get(name).ok_or_else(|| {
                        let known: Vec<&str> = config.segments.keys().map(String::as_str).collect();
                        format!(
                            "segment '{name}' is not in the active config (known: {})",
                            if known.is_empty() {
                                "none declared".to_string()
                            } else {
                                known.join(", ")
                            }
                        )
                    })?;
                    Some((name, predicate.as_str()))
                }
                None => None,
            };

            let inputs = er_cli::PipelineInputs {
                lexicon: &lexicon,
                deliveries: &deliveries,
                model: &model,
                run_id,
                ingested_at: &now,
                scored_at: &now,
                run_at: &now,
                mode: &decision.mode,
                rebuild_reason,
                segment,
            };
            // A declared ensemble routes through `run_all_ensemble`; an empty
            // list goes through `run_all`, which builds the one-member slice
            // itself — so the single-model path stays one call deep and cannot
            // drift from what it did before ensembles existed.
            if members.is_empty() {
                er_cli::run_all(&session, &config, &inputs)
            } else {
                er_cli::run_all_ensemble(&session, &config, &inputs, &members)
            }
            .map_err(|e| format!("{e:#}"))
        }
        other => Err(format!("dispatcher does not handle job kind '{other}'")),
    }?;

    // How many entity events of each kind this run produced, for the webhook
    // aggregate (O3c-2). Read HERE because this is the half that holds the lake
    // session; the half that can emit holds the Postgres client. Thousands of
    // rows become at most one notification per kind, and the detail stays
    // retrievable through the run's entity-event page — notify, do not deliver.
    //
    // Best-effort: the run's results are already written, so failing it because
    // a COUNT could not be read would destroy work to describe it. An empty
    // vector means "nothing to announce", which is also the honest answer for a
    // kind of job that reconciles nothing.
    let counts =
        match er_entities::entity_event::read::counts_by_kind_live(session.connection(), run_id) {
            Ok(rows) => rows
                .into_iter()
                .map(|r| (r.event_type, r.n.unwrap_or(0)))
                .filter(|(_, n)| *n > 0)
                .collect(),
            Err(e) => {
                eprintln!("run {run_id}: entity-event counts unavailable: {e}");
                Vec::new()
            }
        };
    Ok(counts)
}

/// The steward apply path (`apply_corrections` jobs; enqueued by the assertion,
/// unmerge, and bulk-resolve endpoints with `apply_now`). Drains the org's
/// pending staged actions into the lake, then `run_correct` so they take effect
/// over the EXISTING scores — no re-ingest, no re-score. This replaces the old
/// Python `steward.drain_org`, which the all-Rust rebuild left unported: before
/// this, those jobs ran a full re-run that never applied the staged actions.
///
/// Split across the async/sync boundary: the Postgres read/mark are async; the
/// lake apply + correct run on a blocking thread holding the !Send DuckDB
/// session. Idempotent under retry — only actions that actually applied are
/// marked, so a re-drain after a `run_correct` failure finds nothing pending and
/// simply re-corrects.
///
/// # Errors
/// Train a model on the org's corpus and register the result as a DRAFT
/// version (`train` jobs).
///
/// `train` has been in `SCHEDULABLE_KINDS` and enqueued by every tier-C config
/// publish since the registry landed, and the dispatcher had **no arm for it** —
/// so it fell through to "dispatcher does not handle job kind 'train'" and
/// failed every single time. The visible damage was worse than a failed job:
/// publishing a config whose `comparisons` changed enqueues `train` AND a
/// `run_all_full` rebuild, so the rebuild went ahead and re-resolved the whole
/// corpus against the model the publish was meant to replace.
///
/// Two deliberate properties:
///
/// **The new version is a draft, never activated.** Activation is the explicit,
/// audited act the registry exists for, and training is not consent to
/// re-cluster a tenant's data. `er_cli::run_train` is also not cheap, so the
/// operator gets to look at what came out before it decides anything.
///
/// **The TF snapshot is appended, not overwritten.** `build_frozen_tf_tables`
/// filters the CSV by `(model_version, tf_snapshot_id)`, so the new snapshot's
/// rows sit alongside the active model's and each reads only its own. Writing
/// over the file would retune the ACTIVE model's term frequencies underneath it,
/// which is exactly the frozen-TF guarantee that makes incremental runs equal to
/// full ones.
///
/// # Errors
/// Returns a human string (like `run_job`) on a missing input, a training
/// failure, or a registry write failure.
pub async fn train_and_register(
    client: &tokio_postgres::Client,
    org_id: &str,
    env: &Value,
    run_id: &str,
) -> Result<(), String> {
    let catalog = env_str(env, "ER_LAKE_CATALOG")
        .ok_or("org env missing ER_LAKE_CATALOG")?
        .to_string();
    let data_path = env_str(env, "ER_LAKE_DATA_PATH")
        .ok_or("org env missing ER_LAKE_DATA_PATH")?
        .to_string();
    let config_path = env_str(env, "ER_CONFIG_PATH").ok_or("org env missing ER_CONFIG_PATH")?;
    let tf_path = env_str(env, "ER_TF_PATH")
        .ok_or("org env missing ER_TF_PATH")?
        .to_string();
    let config_text = std::fs::read_to_string(config_path)
        .map_err(|e| format!("read config {config_path}: {e}"))?;
    let config = er_config::load_config_str(&config_text)
        .map_err(|e| format!("config: {}", crate::errfmt::chain(&e)))?;

    // The TEMPLATE is the comparison + blocking structure to retrain, taken
    // from the active version so training refines what is live rather than
    // whatever happens to be on disk. Falling back to ER_MODEL_PATH keeps an
    // org created before the registry working.
    let template = match pg::active_model_version(client, org_id).await {
        Ok(Some(m)) => m.model_json,
        Ok(None) => {
            let path = env_str(env, "ER_MODEL_PATH").ok_or(
                "no active model version and no ER_MODEL_PATH, so there is no template to \
                 retrain — publish a model version first",
            )?;
            std::fs::read_to_string(path).map_err(|e| format!("read model template: {e}"))?
        }
        Err(e) => return Err(format!("reading the model registry: {e}")),
    };

    // One label per training run, derived from the run id so two trains cannot
    // collide and so a registry row can be traced back to the job that made it.
    let label = format!("trained-{run_id}");
    let tf_snapshot_id = format!("tf-{run_id}");

    // The sync half: open the lake, train, write the two artifacts.
    // Read the tenant's verdicts here, where the Postgres client is: the
    // training itself runs on a blocking thread with only the lake.
    //
    // MATCHES ONLY. `estimate_m_from_pairwise_labels` reads no verdict column —
    // every pair it is given is counted as a match — so letting a `no_match`
    // through would skew every `m` toward disagreement and move every score the
    // model produces, silently and with no error anywhere.
    let match_labels: Vec<(String, String)> =
        match er_entities::pair_label::read::all(client, org_id).await {
            Ok(rows) => rows
                .into_iter()
                .filter(|l| l.verdict == "match")
                .map(|l| (l.rec_a_key, l.rec_b_key))
                .collect(),
            // A label read that fails must not fail the train: EM is a complete
            // training method on its own, and it is what every run did before
            // labels existed. Reported in provenance as zero labels used.
            Err(e) => {
                eprintln!("reading labels for training failed, falling back to EM-only: {e}");
                Vec::new()
            }
        };
    let labels_used = match_labels.len();

    let (model_json, tf_rows) = {
        let label = label.clone();
        let snap = tf_snapshot_id.clone();
        let training = config.training.clone();
        let match_labels = match_labels.clone();
        tokio::task::spawn_blocking(move || -> Result<(String, String), String> {
            let session =
                er_cli::open_lake(&catalog, &data_path).map_err(|e| format!("open lake: {e:#}"))?;
            let dir = std::env::temp_dir().join(format!("er_train_{label}"));
            std::fs::create_dir_all(&dir).map_err(|e| format!("train scratch dir: {e}"))?;
            let out_model = dir.join("model.json");
            let out_tf = dir.join("tf.csv");
            er_cli::run_train(
                &session,
                &er_cli::TrainInputs {
                    template_settings_json: &template,
                    deterministic_rules: &training.deterministic_rules,
                    em_rules: &training.em_blocking_rules,
                    recall: training.recall,
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a pair budget's magnitude is what matters, and f64 holds \
                                  every value an i64 budget can usefully take"
                    )]
                    u_max_pairs: training.u_max_pairs as f64,
                    model_version: &label,
                    tf_snapshot_id: &snap,
                    match_labels: &match_labels,
                },
                &out_model,
                &out_tf,
            )
            .map_err(|e| format!("training: {e:#}"))?;
            let model_json = std::fs::read_to_string(&out_model)
                .map_err(|e| format!("read trained model: {e}"))?;
            let tf_rows =
                std::fs::read_to_string(&out_tf).map_err(|e| format!("read trained tf: {e}"))?;
            let _ = std::fs::remove_dir_all(&dir);
            Ok((model_json, tf_rows))
        })
        .await
        .map_err(|e| format!("training task panicked: {e}"))??
    };

    // Append this snapshot's rows to the shared frozen-TF CSV, header skipped —
    // see the note above for why appending rather than replacing.
    append_tf_snapshot(&tf_path, &tf_rows)?;

    let provenance = serde_json::json!({
        "source": "train",
        "job_id": run_id,
        "config_hash": er_config::config_hash(&config),
        "trained_from": label,
        // How `m` was estimated, so a version's origin is legible from the
        // registry rather than inferred. Zero means the EM estimate stands —
        // either the tenant has no match verdicts yet, or reading them failed.
        "m_from_labels": labels_used,
    });
    pg::create_model_version(
        client,
        org_id,
        pg::DEFAULT_MODEL_NAME,
        if config.entity_type.is_person() {
            "person"
        } else {
            "account"
        },
        &model_json,
        &label,
        Some(&tf_snapshot_id),
        &provenance,
        "trainer",
    )
    .await
    .map_err(|e| format!("registering the trained model: {e}"))?;
    eprintln!("train {run_id}: registered draft version {label} (not activated)");
    Ok(())
}

/// Append a trained TF snapshot's rows to the shared frozen-TF CSV, writing the
/// header only when the file is new.
///
/// The file is keyed by `(model_version, tf_snapshot_id, column_name)`, so
/// snapshots coexist and each model reads only its own rows.
fn append_tf_snapshot(tf_path: &str, tf_rows: &str) -> Result<(), String> {
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

/// Propagates a Postgres read/mark failure, a lake open/apply failure, or a
/// `run_correct` failure (as a human string, like `run_job` returns).
/// The `provision` job (D8): create the tenant's lake, then promote the org from
/// `provisioning` to `active`.
///
/// `create_org` already did everything that is fast — the org row, the starter
/// config as a published version 1, the one-time admin key — so this is the slow
/// half only. The promotion at the end is what the UI's progress panel polls
/// for, and the e2e spec gives it 180 seconds.
///
/// Idempotent, because a retried job must converge rather than fail: the
/// directory and catalog creation tolerate existing paths, `CREATE DATABASE` is
/// guarded by an existence check, and the promotion is expected-state guarded so
/// a second run on an already-active tenant is a no-op rather than an error.
///
/// # Errors
/// Returns the failure text for the job row; the org is LEFT in `provisioning`
/// so a retry can finish the work. A tenant stuck visibly mid-provision is
/// better than one marked active over a lake that does not exist.
async fn provision_tenant(
    client: &tokio_postgres::Client,
    org: &pg::OrgDetail,
    job_id: &str,
    profile: &Result<Option<crate::provision::Profile>, crate::provision::ProfileError>,
) -> Result<(), String> {
    let profile = match profile {
        Ok(Some(p)) => p,
        // The job outlives the request that made it, so the profile can have
        // been switched off in between. Say so rather than panicking.
        Ok(None) => {
            return Err(
                "auto-provisioning is not configured on this dispatcher (ER_PROVISION_MODE)"
                    .to_string(),
            );
        }
        Err(e) => return Err(format!("auto-provisioning is misconfigured: {e}")),
    };
    let plan = profile.plan(&org.name);

    crate::provision::create_workspace(&plan, &profile.config_template)?;

    match profile.mode {
        crate::provision::Mode::Local => crate::provision::create_local_lake(&plan)?,
        crate::provision::Mode::Postgres => {
            let database = plan
                .tenant
                .as_deref()
                .ok_or("postgres mode produced no tenant database name")?;
            let maint = profile
                .maint_dsn
                .as_deref()
                .ok_or("postgres mode has no ER_PROVISION_MAINT_DSN")?;
            create_tenant_database(maint, database).await?;
        }
    }

    // The lake's tables, at this binary's catalog version, with the ledger
    // saying so — before the org is promoted, so a tenant never goes active
    // over a lake whose tables its first job would have had to create. Opened
    // from the org ROW's env, which is what every later job opens, rather than
    // from the plan re-derived here.
    let env = org.env.clone();
    let schema = tokio::task::spawn_blocking(move || crate::schema::reconcile_lake_env(&env))
        .await
        .unwrap_or_else(|e| Err(format!("dispatcher task panicked: {e}")));
    let (status, hash, detail) = crate::schema::tenant_state(&schema);
    crate::schema::record_tenant_state(client, &org.id, status, &hash, Some(&detail))
        .await
        .map_err(|e| format!("record {}'s schema state: {e}", org.name))?;
    schema.map_err(|e| format!("provision {}'s lake schema: {e}", org.name))?;

    eprintln!(
        "job {job_id}: provisioned {} ({:?})",
        org.name, profile.mode
    );
    // Expected-state guarded: `false` means someone else already moved it (a
    // concurrent retry, or an operator suspending mid-provision), which is not
    // this job's problem to force.
    pg::set_org_state(client, &org.name, "active", "provisioning")
        .await
        .map_err(|e| format!("promote {} to active: {e}", org.name))?;
    Ok(())
}

/// `CREATE DATABASE <db>` on the maintenance connection, if absent.
///
/// The identifier is built by [`crate::provision::tenant_database`], which folds
/// everything but `[a-z0-9]` to `_`, so it cannot carry a quote — but it is still
/// double-quoted here, because `CREATE DATABASE` takes no parameters and a
/// hand-built identifier is exactly the shape that becomes an injection the day
/// someone relaxes the folding.
async fn create_tenant_database(maint_dsn: &str, database: &str) -> Result<(), String> {
    let client = pg::connect(maint_dsn)
        .await
        .map_err(|e| format!("connect to the provisioning maintenance DSN: {e}"))?;
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

pub async fn drain_and_correct(
    client: &tokio_postgres::Client,
    org_id: &str,
    env: &Value,
    run_id: &str,
) -> Result<(), String> {
    let pending = pg::pending_actions(client, org_id)
        .await
        .map_err(|e| format!("read pending actions: {e}"))?;
    // Cloned for the blocking half, which owns what it is handed; the borrowed
    // `env` stays live for the write-back below.
    let env_owned = env.clone();
    let run_id_owned = run_id.to_string();
    let now = fmt_epoch(unix_now());
    let DrainOutcome {
        applied,
        labels,
        resolutions,
        correct,
    } = tokio::task::spawn_blocking(move || {
        apply_and_correct(&env_owned, &pending, &run_id_owned, &now)
    })
    .await
    .unwrap_or_else(|e| DrainOutcome::failed(format!("drain task panicked: {e}")));
    for id in &applied {
        pg::mark_action_applied(client, id)
            .await
            .map_err(|e| format!("mark action applied {id}: {e}"))?;
    }
    // A steward's verdict is a training label (E10b-2). Written here because
    // `pair_label` is a POSTGRES table and the apply half holds the lake
    // session; this is the half with the client.
    //
    // Only for actions that APPLIED. A resolution staged but never drained is
    // not a verdict the engine acted on, and labelling it would train on an
    // intent the corpus never saw.
    //
    // Best-effort and after the marks: the resolution is already in the lake and
    // the corpus is about to be corrected, so failing the drain because a label
    // could not be upserted would undo real work to record an observation about
    // it. `origin = 'reviewer'` has been an accepted value since E10b with
    // nothing ever writing one — the labels API counted a bucket that could not
    // fill.
    for (label, labeled_by) in &labels {
        if let Err(e) = pg::record_reviewer_label(
            client,
            org_id,
            (&label.rec_a_key, &label.rec_b_key),
            label.verdict,
            labeled_by,
        )
        .await
        {
            eprintln!(
                "run {run_id}: reviewer label for {}|{} not recorded: {e}",
                label.rec_a_key, label.rec_b_key
            );
        }
    }
    // `review.resolved` (O3c), for the resolutions that APPLIED. Deliberately
    // not emitted by the resolve endpoint: a resolution is staged and drained
    // later, so an event fired at staging time would tell a subscriber about an
    // intent the engine has not enforced — they would read a corpus that
    // disagrees with the event they just received.
    //
    // Best-effort and after the marks, like the labels above and for the same
    // reason: the decision is in the lake and the corpus has been corrected, so
    // failing the drain to report on it would undo real work.
    if !resolutions.is_empty() {
        let org_name = pg::get_org_by_id(client, org_id)
            .await
            .map_err(|e| format!("look up org {org_id}: {e}"))?
            .map_or_else(|| org_id.to_string(), |o| o.name);
        for (review_id, resolution) in &resolutions {
            if let Err(e) = webhook::emit(
                client,
                org_id,
                &org_name,
                "review.resolved",
                &serde_json::json!({
                    "review_id": review_id,
                    "resolution": resolution,
                    "run_id": run_id,
                }),
            )
            .await
            {
                eprintln!("review.resolved for {review_id} not queued: {e}");
            }
        }
    }
    // The outbound write-back (O4): the resulting MASTER record, not a
    // notification that one changed.
    //
    // Only when the correction succeeded. A write-back describes the corpus
    // after the resolution took effect, so sending one for a correction that
    // failed would hand the tenant's system a master the engine does not
    // believe in — the same reason `review.resolved` fires from the drain
    // rather than from the resolve endpoint, one step further along.
    if correct.is_ok() && !labels.is_empty() {
        write_back_resolutions(client, org_id, env, run_id, &labels).await;
    }
    correct
}

/// Queue one `entity.write_back` per entity a resolution changed.
///
/// Driven off the derived LABELS rather than the raw resolutions, which gets
/// the dismissals right for free: a dismissal yields no label because it is not
/// a verdict about the pair, and it changes no master either, so there is
/// nothing to write back. The labels also carry the canonical pair keys, which
/// is what the lookup needs.
///
/// Both of a pair's keys are resolved, and the entity ids deduplicated: a
/// `match` collapses them to one entity and a `no_match` leaves two, and a
/// subscriber wanting to apply the change physically needs whichever it is
/// without having to work it out.
///
/// Best-effort throughout, like the labels and `review.resolved` before it: the
/// resolution is in the lake and the corpus is corrected, so failing the drain
/// to report on it would undo real work. Every skip is logged with its reason,
/// because a write-back that silently does not happen is indistinguishable from
/// a tenant whose endpoint is fine.
async fn write_back_resolutions(
    client: &tokio_postgres::Client,
    org_id: &str,
    env: &Value,
    run_id: &str,
    labels: &[(er_cli::DerivedLabel, String)],
) {
    // Nothing to do when no subscriber wants it — checked before opening the
    // lake, because the read below costs a session and a snapshot pin.
    match pg::webhooks_for_delivery(client, org_id, "entity.write_back").await {
        Ok(targets) if targets.is_empty() => return,
        Ok(_) => {}
        Err(e) => {
            eprintln!("run {run_id}: write-back subscriber lookup failed: {e}");
            return;
        }
    }
    let Some(catalog) = env_str(env, "ER_LAKE_CATALOG") else {
        eprintln!("run {run_id}: write-back skipped: org env missing ER_LAKE_CATALOG");
        return;
    };
    let Some(data_path) = env_str(env, "ER_LAKE_DATA_PATH") else {
        eprintln!("run {run_id}: write-back skipped: org env missing ER_LAKE_DATA_PATH");
        return;
    };
    let entity_type = crate::entity_type_from_env(env);

    // A fresh session at the LATEST snapshot: the blocking half has committed,
    // so this is the first point at which the corrected corpus is readable.
    let (payloads, org_name) = {
        let session = match er_cli::open_lake(catalog, data_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("run {run_id}: write-back skipped: open lake: {e:#}");
                return;
            }
        };
        let snapshot = match crate::read::latest_snapshot(&session) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("run {run_id}: write-back skipped: {e}");
                return;
            }
        };
        let mut entity_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (label, _) in labels {
            for key in [&label.rec_a_key, &label.rec_b_key] {
                match crate::read::entity_of_record_key(&session, snapshot, key) {
                    Ok(Some(id)) => {
                        entity_ids.insert(id);
                    }
                    // A record the corpus no longer has resolves to nothing.
                    // That is a real outcome (it was deleted between staging and
                    // drain), not an error, and the other half of the pair may
                    // still resolve.
                    Ok(None) => {}
                    Err(e) => eprintln!("run {run_id}: write-back lookup for {key}: {e}"),
                }
            }
        }
        let mut payloads: Vec<(String, Value)> = Vec::new();
        for entity_id in &entity_ids {
            match crate::read::entity_detail(&session, entity_type, snapshot, entity_id) {
                Ok(Some(detail)) => payloads.push((entity_id.clone(), detail)),
                Ok(None) => {}
                Err(e) => eprintln!("run {run_id}: write-back detail for {entity_id}: {e}"),
            }
        }
        let org_name = match pg::get_org_by_id(client, org_id).await {
            Ok(Some(org)) => org.name,
            Ok(None) => org_id.to_string(),
            Err(e) => {
                eprintln!("run {run_id}: write-back skipped: resolving the org: {e}");
                return;
            }
        };
        (payloads, org_name)
    };

    for (entity_id, detail) in payloads {
        let data = serde_json::json!({
            "entity_id": entity_id,
            "run_id": run_id,
            "cause": "review_resolved",
            "master": detail.get("golden").cloned().unwrap_or(Value::Null),
            "members": detail.get("members").cloned().unwrap_or(Value::Null),
        });
        if let Err(e) = webhook::emit(client, org_id, &org_name, "entity.write_back", &data).await {
            eprintln!("run {run_id}: entity.write_back for {entity_id} not queued: {e}");
        }
    }
}

/// What the blocking half of [`drain_and_correct`] produced.
///
/// A struct rather than a 3-tuple because the third element is itself a
/// `Result` and the second a pair — positionally that reads as noise, and
/// clippy says so.
struct DrainOutcome {
    /// Action ids that applied, for the async half to mark.
    applied: Vec<String>,
    /// Training labels a resolution implied, each with the principal that
    /// staged it (E10b-2).
    labels: Vec<(er_cli::DerivedLabel, String)>,
    /// `(review_id, resolution)` for each APPLIED `resolve_review` action, for
    /// the `review.resolved` event (O3c). Includes dismissals: a dismissal is a
    /// decision a subscriber may care about, even though it yields no training
    /// label.
    resolutions: Vec<(String, String)>,
    /// Whether the correction pass that followed succeeded.
    correct: Result<(), String>,
}

impl DrainOutcome {
    /// Nothing applied, nothing labelled, and why.
    fn failed(why: String) -> Self {
        Self {
            applied: Vec::new(),
            labels: Vec::new(),
            resolutions: Vec::new(),
            correct: Err(why),
        }
    }
}

/// Blocking half of [`drain_and_correct`]: apply each pending action to the lake,
/// then correct. Returns the action ids that applied (for the caller to mark)
/// plus the correct outcome. Applying stops at the first failing action (it and
/// the rest stay pending for a later retry); ids that already applied are still
/// returned so they are not re-applied.
fn apply_and_correct(
    env: &Value,
    pending: &[pg::PendingAction],
    run_id: &str,
    now: &str,
) -> DrainOutcome {
    let Some(catalog) = env_str(env, "ER_LAKE_CATALOG") else {
        return DrainOutcome::failed("org env missing ER_LAKE_CATALOG".to_string());
    };
    let Some(data_path) = env_str(env, "ER_LAKE_DATA_PATH") else {
        return DrainOutcome::failed("org env missing ER_LAKE_DATA_PATH".to_string());
    };
    let session = match er_cli::open_lake(catalog, data_path) {
        Ok(s) => s,
        Err(e) => return DrainOutcome::failed(format!("open lake: {e}")),
    };
    let mut applied = Vec::new();
    let mut labels = Vec::new();
    let mut resolutions = Vec::new();
    for staged in pending {
        match er_cli::apply_staged_action_labeled(&session, &staged.id, &staged.action, now) {
            // The actor rides along: a label must name the person who decided,
            // not the role (the D7 lesson).
            Ok(derived) => {
                labels.extend(derived.into_iter().map(|l| (l, staged.created_by.clone())));
            }
            Err(e) => {
                return DrainOutcome {
                    applied,
                    labels,
                    resolutions,
                    correct: Err(format!("apply {}: {e}", staged.id)),
                };
            }
        }
        // Recorded after the apply succeeded, so the event cannot claim a
        // resolution the lake never took.
        if staged.action.get("type").and_then(Value::as_str) == Some("resolve_review") {
            let get = |k: &str| {
                staged
                    .action
                    .get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            resolutions.push((get("review_id"), get("resolution")));
        }
        applied.push(staged.id.clone());
    }
    let config = match env_str(env, "ER_CONFIG_PATH")
        .ok_or_else(|| "org env missing ER_CONFIG_PATH".to_string())
        .and_then(|path| {
            std::fs::read_to_string(path).map_err(|e| format!("read config {path}: {e}"))
        })
        .and_then(|text| {
            er_config::load_config_str(&text)
                .map_err(|e| format!("config: {}", crate::errfmt::chain(&e)))
        }) {
        Ok(c) => c,
        Err(e) => {
            return DrainOutcome {
                applied,
                labels,
                resolutions,
                correct: Err(e),
            };
        }
    };
    let correct = er_cli::run_correct(&session, &config, run_id, now).map_err(|e| format!("{e:#}"));
    DrainOutcome {
        applied,
        labels,
        resolutions,
        correct,
    }
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
