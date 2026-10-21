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

use serde_json::Value;

use crate::{Job, pg, webhook};

const IDLE_MIN: Duration = Duration::from_millis(500);
const IDLE_MAX: Duration = Duration::from_secs(5);

/// How often the scheduler tick runs. Cron's resolution is one minute and the
/// tick is idempotent per minute, so this only has to be comfortably under a
/// minute — it is not the firing precision.
const SCHEDULE_TICK_EVERY: Duration = Duration::from_secs(20);

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
        if dispatch_once(client, http).await? {
            idle = IDLE_MIN; // ran a job; poll again immediately
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
async fn dispatch_once(
    client: &tokio_postgres::Client,
    http: &reqwest::Client,
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

    // The pipeline is synchronous and holds a !Send DuckDB Session, so run it on
    // a blocking thread with owned inputs. The steward apply path is special: it
    // drains the org's pending staged actions into the lake and corrects (reusing
    // the existing scores), which needs the async Postgres client, so it does not
    // go through the blocking `run_job`.
    let env = org.env.clone();
    let outcome = if job.kind == "apply_corrections" {
        drain_and_correct(client, &job.org_id, &env, &job.id).await
    } else {
        // The active model comes from the registry, read here because `run_job`
        // is the blocking half and cannot touch the async Postgres client. An
        // org with no registry version resolves `None` and `run_job` falls back
        // to the env file, which is what every org did before the registry
        // existed.
        let model = match pg::active_model_version(client, &job.org_id).await {
            Ok(m) => m.map(|m| ActiveModel {
                model_json: m.model_json,
                model_version_label: m.model_version_label,
                tf_snapshot_id: m.tf_snapshot_id,
                name: m.name,
                version: m.version,
            }),
            Err(e) => {
                pg::mark(client, &job.id, "failed").await?;
                eprintln!("job {} failed: reading the model registry: {e}", job.id);
                return Ok(true);
            }
        };
        let jb = job.clone();
        let drop_root = org.drop_root.clone();
        tokio::task::spawn_blocking(move || {
            run_job(&jb, &env, drop_root.as_deref(), model.as_ref())
        })
        .await
        .unwrap_or_else(|e| Err(format!("dispatcher task panicked: {e}")))
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
        let _ = webhook::emit_job_event(client, http, &completed).await;
    }
    Ok(true)
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
    active_model: Option<&ActiveModel>,
) -> Result<(), String> {
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

            er_cli::run_all(
                &session,
                &config,
                &er_cli::PipelineInputs {
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
                },
            )
            .map_err(|e| format!("{e:#}"))
        }
        other => Err(format!("dispatcher does not handle job kind '{other}'")),
    }
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
/// Propagates a Postgres read/mark failure, a lake open/apply failure, or a
/// `run_correct` failure (as a human string, like `run_job` returns).
pub async fn drain_and_correct(
    client: &tokio_postgres::Client,
    org_id: &str,
    env: &Value,
    run_id: &str,
) -> Result<(), String> {
    let pending = pg::pending_actions(client, org_id)
        .await
        .map_err(|e| format!("read pending actions: {e}"))?;
    let env = env.clone();
    let run_id_owned = run_id.to_string();
    let now = fmt_epoch(unix_now());
    let (applied, correct): (Vec<String>, Result<(), String>) =
        tokio::task::spawn_blocking(move || apply_and_correct(&env, &pending, &run_id_owned, &now))
            .await
            .unwrap_or_else(|e| (Vec::new(), Err(format!("drain task panicked: {e}"))));
    for id in &applied {
        pg::mark_action_applied(client, id)
            .await
            .map_err(|e| format!("mark action applied {id}: {e}"))?;
    }
    correct
}

/// Blocking half of [`drain_and_correct`]: apply each pending action to the lake,
/// then correct. Returns the action ids that applied (for the caller to mark)
/// plus the correct outcome. Applying stops at the first failing action (it and
/// the rest stay pending for a later retry); ids that already applied are still
/// returned so they are not re-applied.
fn apply_and_correct(
    env: &Value,
    pending: &[(String, Value)],
    run_id: &str,
    now: &str,
) -> (Vec<String>, Result<(), String>) {
    let Some(catalog) = env_str(env, "ER_LAKE_CATALOG") else {
        return (
            Vec::new(),
            Err("org env missing ER_LAKE_CATALOG".to_string()),
        );
    };
    let Some(data_path) = env_str(env, "ER_LAKE_DATA_PATH") else {
        return (
            Vec::new(),
            Err("org env missing ER_LAKE_DATA_PATH".to_string()),
        );
    };
    let session = match er_cli::open_lake(catalog, data_path) {
        Ok(s) => s,
        Err(e) => return (Vec::new(), Err(format!("open lake: {e}"))),
    };
    let mut applied = Vec::new();
    for (action_id, action) in pending {
        if let Err(e) = er_cli::apply_staged_action(&session, action_id, action, now) {
            return (applied, Err(format!("apply {action_id}: {e}")));
        }
        applied.push(action_id.clone());
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
        Err(e) => return (applied, Err(e)),
    };
    let correct = er_cli::run_correct(&session, &config, run_id, now).map_err(|e| format!("{e:#}"));
    (applied, correct)
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
