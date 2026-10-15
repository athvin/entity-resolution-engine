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
    while !term.load(Ordering::SeqCst) {
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

/// Claim + run one job. `Ok(true)` ran a job (outcome persisted), `Ok(false)`
/// the queue was empty, `Err` a Postgres failure (fatal to the caller).
async fn dispatch_once(
    client: &tokio_postgres::Client,
    http: &reqwest::Client,
) -> Result<bool, tokio_postgres::Error> {
    let Some(job) = pg::claim(client).await? else {
        return Ok(false);
    };
    pg::mark(client, &job.job_id, "running").await?;

    let Some(org) = pg::get_org(client, &job.org).await? else {
        pg::mark(client, &job.job_id, "failed").await?;
        eprintln!("job {} failed: org '{}' not found", job.job_id, job.org);
        return Ok(true);
    };

    // The pipeline is synchronous and holds a !Send DuckDB Session, so run it on
    // a blocking thread with owned inputs.
    let jb = job.clone();
    let env = org.env.clone();
    let drop_root = org.drop_root.clone();
    let outcome = tokio::task::spawn_blocking(move || run_job(&jb, &env, drop_root.as_deref()))
        .await
        .unwrap_or_else(|e| Err(format!("dispatcher task panicked: {e}")));

    let state = if outcome.is_ok() {
        "succeeded"
    } else {
        "failed"
    };
    pg::mark(client, &job.job_id, state).await?;
    if let Err(e) = &outcome {
        eprintln!("job {} ({}) failed: {e}", job.job_id, job.kind);
    }
    let completed = Job {
        state: state.to_string(),
        ..job
    };
    let _ = webhook::emit_job_event(client, http, &completed).await;
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
fn run_job(job: &Job, env: &Value, drop_root: Option<&str>) -> Result<(), String> {
    let catalog = env_str(env, "ER_LAKE_CATALOG").ok_or("org env missing ER_LAKE_CATALOG")?;
    let data_path = env_str(env, "ER_LAKE_DATA_PATH").ok_or("org env missing ER_LAKE_DATA_PATH")?;
    let session = er_cli::open_lake(catalog, data_path).map_err(|e| format!("open lake: {e}"))?;
    let run_id = &job.job_id;
    let now = fmt_epoch(unix_now());

    // Config (needed by every kind except lake_maintain).
    let load_config = || -> Result<er_config::Config, String> {
        let path = env_str(env, "ER_CONFIG_PATH").ok_or("org env missing ER_CONFIG_PATH")?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("read config {path}: {e}"))?;
        er_config::load_config_str(&text).map_err(|e| format!("config: {e}"))
    };

    match job.kind.as_str() {
        "lake_maintain" => {
            let expire = env_str(env, "ER_EXPIRE_OLDER_THAN").unwrap_or("7 days");
            er_cli::run_maintain(&session, expire)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        "correct" => {
            let config = load_config()?;
            er_cli::run_correct(&session, &config, run_id, &now).map_err(|e| e.to_string())
        }
        // Incremental and full both resolve + score the delivered corpus; the
        // incremental semantics come from the prior membership in the lake.
        "run_all_full" | "run_all_incremental" => {
            let config = load_config()?;
            let drop_root = drop_root.ok_or("org has no drop_root for a run_all")?;
            let deliveries: Vec<(String, String)> = config
                .sources
                .keys()
                .filter_map(|source| {
                    std::fs::read_to_string(format!("{drop_root}/{source}.csv"))
                        .ok()
                        .map(|csv| (source.clone(), csv))
                })
                .collect();
            if deliveries.is_empty() {
                return Err(format!("no source deliveries found under {drop_root}"));
            }
            let model_path =
                env_str(env, "ER_MODEL_PATH").ok_or("org env missing ER_MODEL_PATH")?;
            let tf_path = env_str(env, "ER_TF_PATH").ok_or("org env missing ER_TF_PATH")?;
            let settings_json =
                std::fs::read_to_string(model_path).map_err(|e| format!("read model: {e}"))?;
            let lexicon = env_str(env, "ER_LEXICON_PATH")
                .map(load_lexicon)
                .unwrap_or_default();
            let model = er_cli::MatchModel {
                settings_json: &settings_json,
                tf_csv_path: Path::new(tf_path),
                model_version: env_str(env, "ER_MODEL_VERSION").unwrap_or("v0001"),
                tf_snapshot_id: env_str(env, "ER_TF_SNAPSHOT_ID").unwrap_or("tf"),
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
                },
            )
            .map_err(|e| e.to_string())
        }
        other => Err(format!("dispatcher does not handle job kind '{other}'")),
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
