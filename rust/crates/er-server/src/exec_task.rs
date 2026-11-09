//! `er-server exec-task` — the pod-side verb (§6.3): fetch the work order over
//! the attempt channel, verify the structural fingerprint against this
//! binary's own registry (nothing about the pipeline crosses the wire), run
//! exactly one try of the task body, and report the outcome through `finish`
//! AND the termination message — the record the watch reads when the report
//! could not land.
//!
//! Synchronous by design: the stage runs on the main thread (DuckDB is
//! synchronous), the HTTP calls ride a current-thread runtime, and the
//! heartbeat and SIGTERM handlers are plain threads. Runs OUTSIDE the server's
//! tokio runtime — `main.rs` dispatches to it before building one.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use er_dag::{CancelToken, CounterSink, TaskCtx, TaskError, WorkOrder};
use er_k8s::{TerminalRecord, TerminalState};

use crate::dags::{self, JobKind};

/// Exit 3 — `ExitCode::Precondition`: the stale-start / mismatch / unreachable
/// family (§6.3; this repo's exit table has no `ResumeRefusal`).
const EXIT_PRECONDITION: i32 = 3;
const CHANNEL_RETRY_ATTEMPTS: usize = 6;
const CHANNEL_RETRY_DELAY: Duration = Duration::from_secs(10);

fn transient_channel_status(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

/// Retry transport faults and temporary HTTP responses; preserve an
/// authoritative response so the caller can report its refusal precisely.
fn request_with_retry(
    operation: &'static str,
    mut request: impl FnMut() -> Result<(u16, Value), String>,
    mut wait: impl FnMut(),
) -> Option<(u16, Value)> {
    for tick in 0..CHANNEL_RETRY_ATTEMPTS {
        match request() {
            Ok((status, body)) if !transient_channel_status(status) => {
                return Some((status, body));
            }
            Ok((status, _)) => {
                tracing::warn!(
                    operation,
                    tick,
                    http.response.status_code = status,
                    "the attempt channel returned a transient response"
                );
            }
            Err(error) => {
                tracing::warn!(operation, tick, %error, "the attempt channel request failed");
            }
        }
        if tick + 1 < CHANNEL_RETRY_ATTEMPTS {
            wait();
        }
    }
    None
}

fn heartbeat_fenced(misses: &mut u32, response: Result<(u16, Value), String>) -> bool {
    match response {
        Ok((200, _)) => {
            *misses = 0;
            false
        }
        Ok((status, _)) if !transient_channel_status(status) => true,
        Ok(_) | Err(_) => {
            *misses += 1;
            *misses >= 3
        }
    }
}

/// The attempt channel's client: one current-thread runtime, one pooled
/// connection, the per-attempt bearer.
struct Channel {
    runtime: tokio::runtime::Runtime,
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Channel {
    fn new(base: String, token: String) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("building the channel runtime: {e}"))?;
        // In-namespace traffic to er-api; redirects are refused like every
        // other client this binary builds.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("building the channel client: {e}"))?;
        Ok(Channel {
            runtime,
            http,
            base,
            token,
        })
    }

    fn get(&self, path: &str) -> Result<(u16, Value), String> {
        let url = format!("{}{path}", self.base);
        self.runtime.block_on(async {
            let resp = self
                .http
                .get(&url)
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(|e| format!("GET {path}: {e}"))?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
        })
    }

    fn post(&self, path: &str, body: &Value) -> Result<(u16, Value), String> {
        let url = format!("{}{path}", self.base);
        self.runtime.block_on(async {
            // The workspace's reqwest carries no `json` feature; the body is
            // serialized by hand.
            let resp = self
                .http
                .post(&url)
                .bearer_auth(&self.token)
                .header("content-type", "application/json")
                .body(body.to_string())
                .send()
                .await
                .map_err(|e| format!("POST {path}: {e}"))?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
        })
    }
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Write the ≤ 4 KiB terminal record on a deliberate exit path (§6.3 step 7).
fn write_termination(record: &TerminalRecord) {
    let path = env_var("ER_TERMINATION_LOG").unwrap_or_else(|| "/dev/termination-log".to_string());
    if let Err(e) = std::fs::write(&path, record.to_capped_json()) {
        tracing::error!(path = %path, error = %e, "the termination record was not written");
    }
}

struct Args {
    attempt: String,
    expect_structural: String,
    #[expect(
        dead_code,
        reason = "segments arrive with Phase 3; the flag is the stable CLI"
    )]
    through: Option<String>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut attempt = None;
    let mut expect = None;
    let mut through = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--attempt" => attempt = Some(it.next().ok_or("--attempt needs a value")?.clone()),
            "--expect-structural" => {
                expect = Some(
                    it.next()
                        .ok_or("--expect-structural needs a value")?
                        .clone(),
                );
            }
            "--through" => through = Some(it.next().ok_or("--through needs a value")?.clone()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(Args {
        attempt: attempt.ok_or("--attempt is required")?,
        expect_structural: expect.ok_or("--expect-structural is required")?,
        through,
    })
}

/// The verb. Returns the process exit code (the er-errors table: 0/1/2/3/10).
#[must_use]
pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(code) => code,
        Err(message) => {
            tracing::error!(error = %message, "the task verb failed");
            EXIT_PRECONDITION
        }
    }
}

#[expect(clippy::too_many_lines, reason = "the §6.3 steps, in order, one place")]
fn run_inner(args: &[String]) -> Result<i32, String> {
    let args = parse_args(args)?;
    let base = env_var("ER_CONTROL_URL").ok_or("ER_CONTROL_URL is required")?;
    let token = match env_var("ER_ATTEMPT_TOKEN_FILE") {
        Some(path) => std::fs::read_to_string(&path)
            .map_err(|e| format!("reading the attempt token at {path}: {e}"))?
            .trim()
            .to_string(),
        None => env_var("ER_ATTEMPT_TOKEN").ok_or(
            "no attempt credential: set ER_ATTEMPT_TOKEN_FILE (the projected volume) or \
             ER_ATTEMPT_TOKEN",
        )?,
    };
    let channel = Channel::new(base, token)?;
    let attempt = args.attempt.clone();

    // Step 1: fetch the work order with a bounded six-attempt patience window
    // while the API may be rolling. Unreachable → control_plane_unreachable,
    // exit 3; the watch charges a launch, not a try.
    let order_response = request_with_retry(
        "work order",
        || channel.get(&format!("/v1/internal/attempts/{attempt}")),
        || std::thread::sleep(CHANNEL_RETRY_DELAY),
    );
    let Some((status, order_body)) = order_response else {
        write_termination(&TerminalRecord {
            v: 1,
            attempt: attempt.clone(),
            state: TerminalState::Failed,
            exit: EXIT_PRECONDITION,
            error_class: None,
            termination_reason: Some("control_plane_unreachable".into()),
            detail: Some("the work order remained unavailable after six attempts".into()),
            output: None,
            snapshot_start: None,
            snapshot_end: None,
            counters: None,
        });
        return Ok(EXIT_PRECONDITION);
    };
    if status != 200 {
        let detail = order_body["detail"].as_str().unwrap_or("").to_string();
        write_termination(&TerminalRecord {
            v: 1,
            attempt: attempt.clone(),
            state: TerminalState::Failed,
            exit: EXIT_PRECONDITION,
            error_class: None,
            termination_reason: Some("stale_start".into()),
            detail: Some(format!("work order refused ({status}): {detail}")),
            output: None,
            snapshot_start: None,
            snapshot_end: None,
            counters: None,
        });
        tracing::error!(http.response.status_code = status, detail = %detail, "the work order was refused");
        return Ok(EXIT_PRECONDITION);
    }

    // Step 2: the binary's OWN registry decides the graph; nothing about the
    // pipeline crossed the wire. A stale image refuses to run a graph it does
    // not have.
    let kind_str = order_body["kind"].as_str().unwrap_or_default().to_string();
    let Some(kind) = JobKind::parse(&kind_str) else {
        return Err(format!("the work order names unknown kind '{kind_str}'"));
    };
    let dag = dags::dag_for(kind);
    if dag.structural_fingerprint != args.expect_structural {
        write_termination(&TerminalRecord {
            v: 1,
            attempt: attempt.clone(),
            state: TerminalState::Failed,
            exit: EXIT_PRECONDITION,
            error_class: None,
            termination_reason: Some("structural_mismatch".into()),
            detail: Some(format!(
                "this image compiles '{}' as {} but the launch expected {}",
                kind.dag_name(),
                dag.structural_fingerprint,
                args.expect_structural
            )),
            output: None,
            snapshot_start: None,
            snapshot_end: None,
            counters: None,
        });
        return Ok(EXIT_PRECONDITION);
    }

    let task_id = order_body["task_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let order = WorkOrder {
        org_id: order_body["org_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        dataset_id: order_body["dataset_id"].as_str().map(str::to_string),
        job_id: order_body["job_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        run_id: order_body["run_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        task_id: task_id.clone(),
        kind: kind_str.clone(),
        try_number: order_body["try_number"].as_i64().unwrap_or(1),
        resource_class: order_body["resource_class"].as_str().map(str::to_string),
        segment: order_body["segment"].as_str().map(str::to_string),
        input: order_body["input"].clone(),
        params: order_body["params"].clone(),
        lake_env: order_body["lake_env"].clone(),
        drop_root: order_body["drop_root"].as_str().map(str::to_string),
        models: order_body["models"].clone(),
        labels: order_body["labels"].clone(),
        connection: order_body["connection"].clone(),
    };

    // Step 3: report start. The body carries this pod's identity for the
    // fallback-token path; under the projected token the server IGNORES it and
    // records the identity from the verified claims instead (§6.3 step 3), so
    // what is sent here is advisory, never authority.
    let pod_uid = env_var("ER_POD_UID").unwrap_or_else(|| format!("pod-{attempt}"));
    let start = json!({
        "k8s_job_name": env_var("ER_K8S_JOB_NAME").unwrap_or_else(|| er_k8s::job_name(&attempt, 1)),
        "pod_uid": pod_uid,
        "pod_name": env_var("ER_POD_NAME"),
        "node_name": env_var("ER_NODE_NAME"),
        "image_digest": env_var("ER_IMAGE_DIGEST"),
    });
    match request_with_retry(
        "start",
        || channel.post(&format!("/v1/internal/attempts/{attempt}/start"), &start),
        || std::thread::sleep(CHANNEL_RETRY_DELAY),
    ) {
        Some((200, _)) => {}
        Some((status, body)) => {
            let detail = body["detail"].as_str().unwrap_or("").to_string();
            write_termination(&TerminalRecord {
                v: 1,
                attempt: attempt.clone(),
                state: TerminalState::Failed,
                exit: EXIT_PRECONDITION,
                error_class: None,
                termination_reason: Some("stale_start".into()),
                detail: Some(format!("start refused ({status}): {detail}")),
                output: None,
                snapshot_start: None,
                snapshot_end: None,
                counters: None,
            });
            tracing::warn!(http.response.status_code = status, detail = %detail, "the start was stale");
            return Ok(EXIT_PRECONDITION);
        }
        None => {
            write_termination(&TerminalRecord {
                v: 1,
                attempt: attempt.clone(),
                state: TerminalState::Failed,
                exit: EXIT_PRECONDITION,
                error_class: None,
                termination_reason: Some("control_plane_unreachable".into()),
                detail: Some("the start remained unavailable after six attempts".into()),
                output: None,
                snapshot_start: None,
                snapshot_end: None,
                counters: None,
            });
            return Ok(EXIT_PRECONDITION);
        }
    }

    // The fence and the heartbeat thread (§6.3 steps 3/6).
    let fence = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let counters = Arc::new(CounterSink::default());
    let cancel = Arc::new(CancelToken::default());
    // SIGTERM: cooperative; the engine-level DuckDB interrupt arrives with
    // Phase 3's long stages (§5.6).
    let sigterm = Arc::new(AtomicBool::new(false));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&sigterm));

    let heartbeat = {
        let fence = Arc::clone(&fence);
        let done = Arc::clone(&done);
        let counters = Arc::clone(&counters);
        let base = channel.base.clone();
        let token = channel.token.clone();
        let attempt = attempt.clone();
        let pod_uid = pod_uid.clone();
        std::thread::spawn(move || {
            let Ok(hb) = Channel::new(base, token) else {
                return;
            };
            let mut misses = 0u32;
            loop {
                // 30 s in 1 s slices so the thread exits promptly when done.
                for _ in 0..30 {
                    if done.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
                let body = json!({ "pod_uid": pod_uid, "counters": counters.snapshot() });
                if heartbeat_fenced(
                    &mut misses,
                    hb.post(&format!("/v1/internal/attempts/{attempt}/heartbeat"), &body),
                ) {
                    // A 409 (or another authoritative refusal) fences now;
                    // transient failures need three consecutive misses.
                    fence.store(true, Ordering::SeqCst);
                    return;
                }
            }
        })
    };

    // Open the session at the class's resource settings (§6.2's env half).
    let session = open_order_session(&order)?;

    // Step 5: run exactly one try.
    let mut ctx = TaskCtx {
        order: &order,
        session: &session,
        counters: &counters,
        fence: &fence,
        cancel: &cancel,
    };
    if sigterm.load(Ordering::SeqCst) {
        cancel.cancel();
    }
    let result = dags::run_pod_task(kind, &task_id, &mut ctx);
    let snapshot_end = session.latest_snapshot_version().ok();
    done.store(true, Ordering::SeqCst);
    let _ = heartbeat.join();

    // Step 6: the fence check before the last write-back. The Phase 2 bodies
    // are idempotent whole-task commits, so the fence here refuses the REPORT:
    // a fenced pod must not post a verdict another try may already own.
    if fence.load(Ordering::SeqCst) {
        let record = TerminalRecord {
            v: 1,
            attempt: attempt.clone(),
            state: TerminalState::Interrupted,
            exit: EXIT_PRECONDITION,
            error_class: None,
            termination_reason: Some("fenced".into()),
            detail: Some("the heartbeat lost the row; no verdict posted".into()),
            output: None,
            snapshot_start: None,
            snapshot_end,
            counters: counters.snapshot(),
        };
        write_termination(&record);
        return Ok(EXIT_PRECONDITION);
    }
    if sigterm.load(Ordering::SeqCst) && result.is_err() {
        // An error under SIGTERM is an interruption, not a verdict (§5.6): the
        // watch decides from the platform's evidence.
        let record = TerminalRecord {
            v: 1,
            attempt: attempt.clone(),
            state: TerminalState::Interrupted,
            exit: EXIT_PRECONDITION,
            error_class: None,
            termination_reason: Some("sigterm".into()),
            detail: None,
            output: None,
            snapshot_start: None,
            snapshot_end,
            counters: counters.snapshot(),
        };
        let _ = channel.post(
            &format!("/v1/internal/attempts/{attempt}/finish"),
            &json!({
                "state": "interrupted",
                "pod_uid": pod_uid,
                "termination_reason": "sigterm",
                "counters": counters.snapshot(),
            }),
        );
        write_termination(&record);
        return Ok(EXIT_PRECONDITION);
    }

    // Step 7: the terminal report, to the channel AND the termination message.
    match result {
        Ok(run) => {
            let exit = if run.outcome == "no_op" { 10 } else { 0 };
            let finish = json!({
                "state": if run.skipped { "skipped" } else { "succeeded" },
                "pod_uid": pod_uid,
                "outcome": run.outcome,
                "output": run.output,
                "counters": run.counters,
                "snapshot_end": snapshot_end,
                "exit_code": exit,
            });
            let posted = channel.post(&format!("/v1/internal/attempts/{attempt}/finish"), &finish);
            if let Err(e) = &posted {
                tracing::warn!(error = %e, "the finish was not posted; the termination record carries the verdict");
            }
            write_termination(&TerminalRecord {
                v: 1,
                attempt,
                state: if run.skipped {
                    TerminalState::Skipped
                } else {
                    TerminalState::Succeeded
                },
                exit,
                error_class: None,
                termination_reason: None,
                detail: None,
                output: Some(run.output),
                snapshot_start: None,
                snapshot_end,
                counters: run.counters,
            });
            Ok(exit)
        }
        Err(TaskError::Skip) => {
            let finish = json!({
                "state": "skipped",
                "pod_uid": pod_uid,
                "outcome": "done",
                "snapshot_end": snapshot_end,
                "exit_code": 0,
            });
            let _ = channel.post(&format!("/v1/internal/attempts/{attempt}/finish"), &finish);
            write_termination(&TerminalRecord {
                v: 1,
                attempt,
                state: TerminalState::Skipped,
                exit: 0,
                error_class: None,
                termination_reason: None,
                detail: None,
                output: None,
                snapshot_start: None,
                snapshot_end,
                counters: counters.snapshot(),
            });
            Ok(0)
        }
        Err(TaskError::Failed(err)) => {
            let class = err.classify();
            let exit = err.code();
            let detail = crate::errfmt::chain(&err);
            let finish = json!({
                "state": "failed",
                "pod_uid": pod_uid,
                "error_class": class.as_str(),
                "error_detail": detail,
                "exit_code": exit,
                "counters": counters.snapshot(),
            });
            let _ = channel.post(&format!("/v1/internal/attempts/{attempt}/finish"), &finish);
            write_termination(&TerminalRecord {
                v: 1,
                attempt,
                state: TerminalState::Failed,
                exit,
                error_class: Some(class.as_str().to_string()),
                termination_reason: None,
                detail: Some(detail.clone()),
                output: None,
                snapshot_start: None,
                snapshot_end,
                counters: counters.snapshot(),
            });
            tracing::error!(er.error_class = %class.as_str(), detail = %detail, "the task failed");
            Ok(exit)
        }
    }
}

/// The lake session from the work order's tenant env plus the class's resource
/// settings (`ER_DUCKDB_THREADS` / `ER_DUCKDB_MEMORY_LIMIT` from the pod env —
/// the same template operation as `requests.cpu`, §6.2), subject to the §9.3
/// determinism gate those fixtures now hold open.
fn open_order_session(order: &WorkOrder) -> Result<dedupe_core::Session, String> {
    let catalog = order.lake_env["ER_LAKE_CATALOG"]
        .as_str()
        .ok_or("the work order's lake env is missing ER_LAKE_CATALOG")?;
    let data_path = order.lake_env["ER_LAKE_DATA_PATH"]
        .as_str()
        .ok_or("the work order's lake env is missing ER_LAKE_DATA_PATH")?;
    let mut options = dedupe_core::session::SessionOptions::deterministic().with_baked_extensions();
    if let Some(threads) = env_var("ER_DUCKDB_THREADS").and_then(|v| v.parse::<u32>().ok()) {
        options = options.with_threads(threads);
    }
    if let Some(limit) = env_var("ER_DUCKDB_MEMORY_LIMIT") {
        options = options.with_memory_limit(limit);
    }
    let mut session = dedupe_core::Session::open_in_memory(options)
        .map_err(|e| format!("open session: {e:#}"))?;
    session
        .attach_ducklake(&dedupe_core::session::ducklake::DuckLakeConfig {
            metadata: catalog.to_string(),
            data_path: Some(data_path.to_string()),
            ..dedupe_core::session::ducklake::DuckLakeConfig::default()
        })
        .map_err(|e| format!("open lake: {e:#}"))?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_channel_responses_retry_but_conflict_is_terminal() {
        let mut responses = [
            Ok((500, Value::Null)),
            Ok((503, Value::Null)),
            Ok((200, json!({ "ok": true }))),
        ]
        .into_iter();
        let mut waits = 0;
        let result = request_with_retry(
            "test",
            || responses.next().expect("request budget"),
            || waits += 1,
        );
        assert_eq!(result, Some((200, json!({ "ok": true }))));
        assert_eq!(waits, 2);

        let mut calls = 0;
        let result = request_with_retry(
            "test",
            || {
                calls += 1;
                Ok((409, json!({ "detail": "fenced" })))
            },
            || panic!("a conflict must not be retried"),
        );
        assert_eq!(result, Some((409, json!({ "detail": "fenced" }))));
        assert_eq!(calls, 1);
    }

    #[test]
    fn transient_channel_exhaustion_is_bounded() {
        let mut calls = 0;
        let mut waits = 0;
        let result = request_with_retry(
            "test",
            || {
                calls += 1;
                Ok((503, Value::Null))
            },
            || waits += 1,
        );
        assert_eq!(result, None);
        assert_eq!(calls, CHANNEL_RETRY_ATTEMPTS);
        assert_eq!(waits, CHANNEL_RETRY_ATTEMPTS - 1);
    }

    #[test]
    fn heartbeat_transient_misses_reset_on_success_and_conflict_fences_immediately() {
        let mut misses = 0;
        assert!(!heartbeat_fenced(&mut misses, Ok((500, Value::Null))));
        assert_eq!(misses, 1);
        assert!(!heartbeat_fenced(&mut misses, Ok((503, Value::Null))));
        assert_eq!(misses, 2);
        assert!(!heartbeat_fenced(&mut misses, Ok((200, Value::Null))));
        assert_eq!(misses, 0);
        assert!(!heartbeat_fenced(&mut misses, Err("timeout".into())));
        assert!(!heartbeat_fenced(&mut misses, Ok((503, Value::Null))));
        assert!(heartbeat_fenced(&mut misses, Ok((500, Value::Null))));

        let mut misses = 0;
        assert!(heartbeat_fenced(&mut misses, Ok((409, Value::Null))));
        assert_eq!(misses, 0);
    }
}
