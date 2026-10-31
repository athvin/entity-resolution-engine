//! The task-ledger decisions that are MORE than one statement, each run in one
//! Postgres transaction on the caller's connection (§4.5): planning (claim →
//! rows → job), `fail_or_retry` (archive + successor), `clear` (three arms +
//! reopen) and the fused finish-and-advance. The framework has no
//! multi-statement form, so a decision is two (or five) declared files on one
//! transaction — `pg_task_instances.rs` pins each pair.
//!
//! Single statements stay thin wrappers over the generated functions, the way
//! `pg.rs` wraps the job statements.

use er_entities::job::Job;
use er_entities::task_instance::TaskInstance;
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;

use er_dag::{ClassRule, Dag, NodeKind};

type PgError = tokio_postgres::Error;

/// How many Kubernetes launches one try may consume (§5.5).
pub const DEFAULT_MAX_LAUNCHES: i64 = 3;
/// Pod-task backoff (§5.5): a cold launch costs ~30 s, so today's 5 s job
/// base is noise at pod granularity.
pub const POD_BACKOFF: (i64, i64) = (30, 900);
/// Control-task backoff (§5.5).
pub const CONTROL_BACKOFF: (i64, i64) = (5, 60);

/// The attempt id: a pure function of the attempt key, so the Job name is one
/// too (§4.2).
#[must_use]
pub fn attempt_id(job_id: &str, task_id: &str, try_number: i64) -> String {
    er_ids::natural_id(
        "task_instances",
        &[job_id, task_id, &try_number.to_string()],
    )
}

/// Planning is one transaction (§4.5): one `plan.sql` row per compiled node
/// (idempotent — re-planning after a clear inserts nothing for rows that
/// exist), then `job/update/plan.sql` (`dispatching → running`, `run_id = id`,
/// both hashes). A crash mid-planning rolls back to `queued` with zero task
/// rows.
///
/// # Errors
/// A statement failed; the transaction rolled back.
pub async fn plan(
    client: &mut Client,
    job: &Job,
    dag: &Dag,
    owner_epoch: &str,
    image_digest: Option<&str>,
) -> Result<Job, PgError> {
    let tx = client.transaction().await?;
    for (seq, node) in dag.nodes.iter().enumerate() {
        let id = attempt_id(&job.id, node.task_id, 1);
        let upstream = serde_json::Value::Array(
            node.upstream
                .iter()
                .map(|u| serde_json::Value::String(dag.nodes[u.index()].task_id.to_string()))
                .collect(),
        );
        // Phase 1 class resolution: an operator override on the job row, else
        // M — the default before any run has completed. The last-succeeded
        // `init` lookup (§3.7) arrives with the executor.
        let resource_class = match node.kind {
            NodeKind::Control => None,
            NodeKind::Pod => Some(match node.policy.class {
                ClassRule::Fixed(c) => c.as_str(),
                ClassRule::FromRun => job
                    .resource_class
                    .as_deref()
                    .filter(|c| matches!(*c, "S" | "M" | "L"))
                    .unwrap_or("M"),
            }),
        };
        er_entities::task_instance::create::plan(
            &tx,
            &id,
            &job.org_id,
            &job.id,
            node.task_id,
            i64::try_from(seq).expect("a DAG is at most a few nodes"),
            node.kind.as_str(),
            node.trigger_rule.as_str(),
            &upstream,
            node.teardown,
            node.policy.segment,
            resource_class,
            i64::from(node.policy.retries),
            DEFAULT_MAX_LAUNCHES,
            image_digest,
            job.trace_context.as_deref(),
        )
        .await?;
    }
    let planned = er_entities::job::update::plan(
        &tx,
        &dag.structural_fingerprint,
        &dag.policy_hash,
        image_digest,
        owner_epoch,
        &job.id,
    )
    .await?;
    tx.commit().await?;
    Ok(planned
        .into_iter()
        .next()
        .expect("plan.sql matches the job claim.sql just handed out"))
}

/// A planning refusal — a kind that does not parse, an assembly failure — is
/// terminal in the same breath (§4.5).
///
/// # Errors
/// The statement failed.
pub async fn refuse_plan(
    client: &Client,
    job_id: &str,
    error_class: &str,
    error_detail: &str,
) -> Result<Option<Job>, PgError> {
    let truncated: String = error_detail.chars().take(2000).collect();
    let rows = er_entities::job::update::refuse_plan(
        client,
        Some(error_class),
        Some(truncated.as_str()),
        job_id,
    )
    .await?;
    Ok(rows.into_iter().next())
}

/// Everything `fail_or_retry` needs to decide and record (§4.5/§5.5).
#[derive(Debug, Clone, Copy)]
pub struct TaskFailure<'a> {
    /// `failed` | `timed_out`.
    pub terminal_state: &'a str,
    pub retryable: bool,
    /// Retry at the next class up (an OOM that never committed).
    pub escalate: bool,
    pub exit_code: Option<i64>,
    pub termination_reason: Option<&'a str>,
    pub error_class: Option<&'a str>,
    pub error_detail: Option<&'a str>,
    /// `None` for a control row (`IS NOT DISTINCT FROM` in the guard).
    pub k8s_job_name: Option<&'a str>,
    /// `POD_BACKOFF` or `CONTROL_BACKOFF`.
    pub backoff: (i64, i64),
}

/// `fail_or_retry` = `archive_try` + `insert_retry_try`, one transaction
/// (§4.5): archive the live row forced to a failure-like terminal state, and
/// when the archive came back `live IS NULL`, insert the successor with the
/// budget arithmetic and deterministic jitter. Returns `(archived, successor)`
/// — `(None, None)` is the stale-report no-op.
///
/// # Errors
/// A statement failed; the transaction rolled back.
pub async fn fail_or_retry_task(
    client: &mut Client,
    task_instance_id: &str,
    failure: &TaskFailure<'_>,
) -> Result<(Option<TaskInstance>, Option<TaskInstance>), PgError> {
    let truncated: Option<String> = failure.error_detail.map(|d| d.chars().take(2000).collect());
    let tx = client.transaction().await?;
    let archived = er_entities::task_instance::update::archive_try(
        &tx,
        failure.terminal_state,
        failure.retryable,
        failure.exit_code,
        failure.termination_reason,
        failure.error_class,
        truncated.as_deref(),
        task_instance_id,
        failure.k8s_job_name,
    )
    .await?
    .into_iter()
    .next();
    let successor = match &archived {
        Some(row) if row.live.is_none() => er_entities::task_instance::create::insert_retry_try(
            &tx,
            failure.escalate,
            failure.backoff.1,
            failure.backoff.0,
            task_instance_id,
        )
        .await?
        .into_iter()
        .next(),
        _ => None,
    };
    tx.commit().await?;
    Ok((archived, successor))
}

/// What a clear did (§5.4).
#[derive(Debug, Default)]
pub struct Cleared {
    /// Live rows now `restarting` (the watch rotates them).
    pub restarting: Vec<TaskInstance>,
    /// Successors of rotated terminal rows, in `none`.
    pub rotated: Vec<TaskInstance>,
    /// Pending rows reset in place.
    pub reset: Vec<TaskInstance>,
    /// The reopened job.
    pub job: Option<Job>,
}

/// Why a clear was refused.
#[derive(Debug, thiserror::Error)]
pub enum ClearError {
    #[error("the job is canceling; a clear under a cancel is refused")]
    Canceling,
    #[error("clearing the job's task rows")]
    Pg(#[from] PgError),
}

/// `clear` = `restart_cleared` + `archive_cleared` (+ one
/// `insert_cleared_try` per archived row) + `reset_pending` + `reopen`, in one
/// transaction opened by `job/read/lock.sql` (§4.5). The closure itself —
/// which task ids — is the caller's (`er_dag::clear_closure`).
///
/// # Errors
/// Refused under a canceling job, or a statement failed (rolled back).
pub async fn clear(
    client: &mut Client,
    org_id: &str,
    job_id: &str,
    task_ids: &[String],
    retries: i64,
    resource_class: Option<&str>,
    image_digest: Option<&str>,
) -> Result<Cleared, ClearError> {
    let ids = serde_json::Value::Array(
        task_ids
            .iter()
            .map(|t| serde_json::Value::String(t.clone()))
            .collect(),
    );
    let tx = client.transaction().await?;
    let locked = er_entities::job::read::lock(&tx, job_id).await?;
    let Some(job) = locked.into_iter().next() else {
        return Ok(Cleared::default());
    };
    if job.state == "canceling" {
        return Err(ClearError::Canceling);
    }
    let restarting = er_entities::task_instance::update::restart_cleared(
        &tx,
        resource_class,
        org_id,
        job_id,
        &ids,
    )
    .await?;
    let archived =
        er_entities::task_instance::update::archive_cleared(&tx, org_id, job_id, &ids).await?;
    let mut rotated = Vec::new();
    for row in &archived {
        rotated.extend(
            er_entities::task_instance::create::insert_cleared_try(
                &tx,
                resource_class,
                retries,
                image_digest,
                &row.id,
            )
            .await?,
        );
    }
    let reset = er_entities::task_instance::update::reset_pending(
        &tx,
        retries,
        resource_class,
        image_digest,
        org_id,
        job_id,
        &ids,
    )
    .await?;
    let job = er_entities::job::update::reopen(&tx, job_id, org_id)
        .await?
        .into_iter()
        .next();
    tx.commit().await?;
    Ok(Cleared {
        restarting,
        rotated,
        reset,
        job,
    })
}

/// The fused pod's hand-off (§3.7): `finish.sql` for member *k* and
/// `advance_segment.sql` for *k+1*, one transaction opened by
/// `job/read/lock.sql`, finish first — so the one-pod index sees *k* leave its
/// predicate before *k+1* enters it. Returns `(finished, advanced)`;
/// `advanced = None` reports "no advance" to the pod.
///
/// # Errors
/// A statement failed; the transaction rolled back.
#[allow(clippy::too_many_arguments)] // the two statements' parameter sets, verbatim
pub async fn finish_and_advance(
    client: &mut Client,
    finished_id: &str,
    pod_uid: &str,
    skipped: bool,
    outcome: &str,
    output: Option<&serde_json::Value>,
    snapshots: (Option<i64>, Option<i64>),
    counters: Option<&serde_json::Value>,
    exit_code: Option<i64>,
    next: Option<&AdvanceArgs<'_>>,
) -> Result<(Option<TaskInstance>, Option<TaskInstance>), PgError> {
    let tx = client.transaction().await?;
    if let Some(advance) = next {
        // The pass, the clear and this hand-off serialize on the job row
        // (§4.5).
        let _lock = er_entities::job::read::lock(&tx, advance.job_id).await?;
    }
    let finished = er_entities::task_instance::update::finish(
        &tx,
        skipped,
        outcome,
        output,
        snapshots.0,
        snapshots.1,
        counters,
        exit_code,
        finished_id,
        pod_uid,
    )
    .await?
    .into_iter()
    .next();
    let advanced = match (&finished, next) {
        (Some(done), Some(advance)) => er_entities::task_instance::update::advance_segment(
            &tx,
            done.k8s_job_name.as_deref().unwrap_or_default(),
            done.owner_epoch.as_deref().unwrap_or_default(),
            done.pod_name.as_deref(),
            pod_uid,
            done.node_name.as_deref(),
            done.image_digest.as_deref(),
            advance.input,
            advance.next_task_instance_id,
            advance.job_id,
            advance.segment,
            advance.resource_class,
            advance.clear_number,
        )
        .await?
        .into_iter()
        .next(),
        _ => None,
    };
    tx.commit().await?;
    Ok((finished, advanced))
}

/// The *k+1* half of [`finish_and_advance`], composed by the handler from the
/// compiled graph exactly as the readiness pass would (§3.7).
#[derive(Debug)]
pub struct AdvanceArgs<'a> {
    pub next_task_instance_id: &'a str,
    pub job_id: &'a str,
    pub segment: &'a str,
    pub resource_class: &'a str,
    pub clear_number: i64,
    pub input: &'a serde_json::Value,
}

/// `claim_launch`'s unique-violation on the one-pod-per-org index maps to
/// "nothing to claim", exactly as `pg::claim` maps the jobs index (§4.5).
///
/// # Errors
/// A non-unique-violation query error.
pub async fn claim_launch(
    client: &Client,
    owner_epoch: &str,
    k8s_job_name: &str,
    token_hash: Option<&str>,
    in_flight_cap: i64,
) -> Result<Option<TaskInstance>, PgError> {
    match er_entities::task_instance::update::claim_launch(
        client,
        owner_epoch,
        k8s_job_name,
        token_hash,
        in_flight_cap,
    )
    .await
    {
        Ok(rows) => Ok(rows.into_iter().next()),
        Err(e) if e.code() == Some(&SqlState::UNIQUE_VIOLATION) => Ok(None),
        Err(e) => Err(e),
    }
}
