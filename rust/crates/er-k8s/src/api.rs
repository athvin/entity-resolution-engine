//! The one seam the dispatcher launches and watches through (§6.9): a trait
//! over the five Kubernetes verbs the Role grants, with [`KubeApi`] as the only
//! production implementation — always compiled, no Cargo feature — and
//! er-server's `fake_pod_api` test double scripted against the same shapes.
//! `tests/no_second_executor.rs` (er-server) is the gate that keeps the double
//! out of the binary.

use std::collections::BTreeMap;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Event, Pod};

/// The evidence-bearing snapshot of a `batch/v1 Job`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobSnapshot {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub active: i32,
    pub succeeded: i32,
    pub failed: i32,
    /// Condition `Failed` with `reason: DeadlineExceeded`.
    pub failed_deadline: bool,
    /// Condition `Failed` with `reason: BackoffLimitExceeded`.
    pub failed_backoff: bool,
    pub deletion_timestamp: bool,
}

impl JobSnapshot {
    #[must_use]
    pub fn from_job(job: &Job) -> Self {
        let meta = &job.metadata;
        let status = job.status.as_ref();
        let mut failed_deadline = false;
        let mut failed_backoff = false;
        if let Some(conditions) = status.and_then(|s| s.conditions.as_ref()) {
            for c in conditions {
                if c.type_ == "Failed" && c.status == "True" {
                    match c.reason.as_deref() {
                        Some("DeadlineExceeded") => failed_deadline = true,
                        Some("BackoffLimitExceeded") => failed_backoff = true,
                        _ => {}
                    }
                }
            }
        }
        JobSnapshot {
            name: meta.name.clone().unwrap_or_default(),
            labels: meta
                .labels
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            annotations: meta
                .annotations
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            active: status.and_then(|s| s.active).unwrap_or(0),
            succeeded: status.and_then(|s| s.succeeded).unwrap_or(0),
            failed: status.and_then(|s| s.failed).unwrap_or(0),
            failed_deadline,
            failed_backoff,
            deletion_timestamp: meta.deletion_timestamp.is_some(),
        }
    }
}

/// The pod lifecycle phases this crate distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodPhase {
    Pending,
    Running,
    Succeeded,
    Failed,
    Unknown,
}

/// The evidence-bearing snapshot of a Pod (§6.5's inputs).
#[derive(Debug, Clone, PartialEq)]
pub struct PodSnapshot {
    pub name: String,
    pub uid: String,
    pub node_name: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub phase: PodPhase,
    /// The task container's `waiting.reason`, while pre-start.
    pub waiting_reason: Option<String>,
    pub terminated_exit_code: Option<i32>,
    /// `terminated.reason` (`OOMKilled`, …).
    pub terminated_reason: Option<String>,
    /// `terminated.message` — the terminal record's transport.
    pub termination_message: Option<String>,
    /// The `DisruptionTarget` condition's reason, when present.
    pub disruption_target: Option<String>,
    /// `status.reason` (`Evicted`, `NodeLost`, …).
    pub status_reason: Option<String>,
    /// `PodScheduled=False` with `reason: Unschedulable`.
    pub unschedulable: bool,
    /// The `PodScheduled=False` message plus the newest FailedScheduling text.
    pub scheduling_message: Option<String>,
    /// `Ready=False` with `reason: NodeNotReady` — the platform is ending it.
    pub node_not_ready: bool,
}

impl PodSnapshot {
    #[must_use]
    pub fn from_pod(pod: &Pod) -> Self {
        let meta = &pod.metadata;
        let status = pod.status.as_ref();
        let phase = match status.and_then(|s| s.phase.as_deref()) {
            Some("Running") => PodPhase::Running,
            Some("Succeeded") => PodPhase::Succeeded,
            Some("Failed") => PodPhase::Failed,
            Some("Pending") | None => PodPhase::Pending,
            Some(_) => PodPhase::Unknown,
        };
        let mut waiting_reason = None;
        let mut terminated_exit_code = None;
        let mut terminated_reason = None;
        let mut termination_message = None;
        if let Some(statuses) = status.and_then(|s| s.container_statuses.as_ref()) {
            for cs in statuses {
                if let Some(state) = &cs.state {
                    if let Some(w) = &state.waiting {
                        waiting_reason.clone_from(&w.reason);
                    }
                    if let Some(t) = &state.terminated {
                        terminated_exit_code = Some(t.exit_code);
                        terminated_reason.clone_from(&t.reason);
                        termination_message.clone_from(&t.message);
                    }
                }
            }
        }
        let mut disruption_target = None;
        let mut unschedulable = false;
        let mut scheduling_message = None;
        let mut node_not_ready = false;
        if let Some(conditions) = status.and_then(|s| s.conditions.as_ref()) {
            for c in conditions {
                match c.type_.as_str() {
                    "DisruptionTarget" if c.status == "True" => {
                        disruption_target = Some(
                            c.reason
                                .clone()
                                .unwrap_or_else(|| "DisruptionTarget".into()),
                        );
                    }
                    "PodScheduled" if c.status == "False" => {
                        if c.reason.as_deref() == Some("Unschedulable") {
                            unschedulable = true;
                        }
                        scheduling_message.clone_from(&c.message);
                    }
                    "Ready"
                        if c.status == "False" && c.reason.as_deref() == Some("NodeNotReady") =>
                    {
                        node_not_ready = true;
                    }
                    _ => {}
                }
            }
        }
        PodSnapshot {
            name: meta.name.clone().unwrap_or_default(),
            uid: meta.uid.clone().unwrap_or_default(),
            node_name: pod.spec.as_ref().and_then(|s| s.node_name.clone()),
            labels: meta
                .labels
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            annotations: meta
                .annotations
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            phase,
            waiting_reason,
            terminated_exit_code,
            terminated_reason,
            termination_message,
            disruption_target,
            status_reason: status.and_then(|s| s.reason.clone()),
            unschedulable,
            scheduling_message,
            node_not_ready,
        }
    }

    /// The pod's phase is terminal (clause d of §6.5: a verdict that ends in a
    /// relaunch is written only on terminal-phase or DELETED evidence).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, PodPhase::Succeeded | PodPhase::Failed)
    }
}

/// A namespace Event's evidence (`FailedCreate`, `FailedScheduling`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSnapshot {
    pub reason: String,
    pub message: String,
}

impl EventSnapshot {
    #[must_use]
    pub fn from_event(event: &Event) -> Self {
        EventSnapshot {
            reason: event.reason.clone().unwrap_or_default(),
            message: event.message.clone().unwrap_or_default(),
        }
    }
}

/// What a LIST under the managed selector returns.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub jobs: Vec<JobSnapshot>,
    pub pods: Vec<PodSnapshot>,
}

/// One frame of the two-stream watch.
#[derive(Debug, Clone)]
pub enum WatchFrame {
    Job(JobSnapshot),
    JobDeleted(JobSnapshot),
    Pod(PodSnapshot),
    PodDeleted(PodSnapshot),
    /// The stream (re)initialised: the consumer should reconcile from a LIST.
    Resync,
}

/// What a label patch targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Job,
    Pod,
}

/// A label selector (the one selector the design uses: managed=task,done!=true).
#[derive(Debug, Clone)]
pub struct Selector(pub String);

/// The seam. Object-safe: the dispatcher holds `&dyn PodApi`.
pub trait PodApi: Send + Sync {
    /// Idempotent on the Job name (§6.5's create-outcomes contract).
    fn create_job(&self, job: &Job) -> BoxFuture<'_, Result<(), ApiError>>;
    /// `Ok(None)` on 404 — the probe the §6.5 create-outcome resolution needs.
    fn get_job(&self, name: &str) -> BoxFuture<'_, Result<Option<JobSnapshot>, ApiError>>;
    /// `FailedCreate` / `FailedScheduling` messages about one object, newest
    /// last; read on demand, never a third watch stream (§6.7).
    fn list_events(&self, about: &str) -> BoxFuture<'_, Result<Vec<EventSnapshot>, ApiError>>;
    fn list(&self, selector: &Selector) -> BoxFuture<'_, Result<Listing, ApiError>>;
    /// Labels-only merge patch (`patch` is the only write verb on pods, §6.7).
    fn patch_labels(
        &self,
        kind: ObjectKind,
        name: &str,
        labels: &[(&str, Option<&str>)],
    ) -> BoxFuture<'_, Result<(), ApiError>>;
    /// Foreground propagation: the delete returns when the deletion timestamp
    /// is set, and the pod still has its grace (§6.5 clause d).
    fn delete_job(&self, name: &str) -> BoxFuture<'_, Result<(), ApiError>>;
    fn watch(&self, selector: &Selector) -> BoxStream<'_, WatchFrame>;
}

/// A platform error, classified only as far as the dispatcher's decisions need.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("the API refused the write for capacity: {message}")]
    Quota { message: String },
    #[error("the object already exists")]
    AlreadyExists,
    #[error("rate limited; retry after {retry_after_secs:?}s")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("the API call failed ({context})")]
    Other {
        context: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ApiError {
    fn from_kube(context: &'static str, e: kube::Error) -> Self {
        if let kube::Error::Api(ae) = &e {
            // `exceeded quota` is the one deliberate message-text match in the
            // design (§6.5); the Event carries only the message.
            if ae.code == 403 && ae.message.contains("exceeded quota") {
                return ApiError::Quota {
                    message: ae.message.clone(),
                };
            }
            if ae.code == 409 && ae.reason == "AlreadyExists" {
                return ApiError::AlreadyExists;
            }
            if ae.code == 429 {
                return ApiError::RateLimited {
                    retry_after_secs: None,
                };
            }
        }
        ApiError::Other {
            context,
            source: Box::new(e),
        }
    }
}

/// The production implementation: one namespace, one `kube::Client`.
pub struct KubeApi {
    jobs: kube::Api<Job>,
    pods: kube::Api<Pod>,
    events: kube::Api<Event>,
}

impl KubeApi {
    /// In-cluster configuration (the dispatcher pod's own ServiceAccount).
    ///
    /// # Errors
    /// No in-cluster environment, or the client could not be built.
    pub fn in_cluster(namespace: &str) -> Result<Self, ApiError> {
        let config = kube::Config::incluster().map_err(|e| ApiError::Other {
            context: "in-cluster config",
            source: Box::new(e),
        })?;
        let client = kube::Client::try_from(config).map_err(|e| ApiError::Other {
            context: "kube client",
            source: Box::new(e),
        })?;
        Ok(KubeApi {
            jobs: kube::Api::namespaced(client.clone(), namespace),
            pods: kube::Api::namespaced(client.clone(), namespace),
            events: kube::Api::namespaced(client, namespace),
        })
    }
}

impl PodApi for KubeApi {
    fn create_job(&self, job: &Job) -> BoxFuture<'_, Result<(), ApiError>> {
        let job = job.clone();
        Box::pin(async move {
            self.jobs
                .create(&kube::api::PostParams::default(), &job)
                .await
                .map(|_| ())
                .map_err(|e| ApiError::from_kube("create job", e))
        })
    }

    fn get_job(&self, name: &str) -> BoxFuture<'_, Result<Option<JobSnapshot>, ApiError>> {
        let name = name.to_string();
        Box::pin(async move {
            match self.jobs.get_opt(&name).await {
                Ok(job) => Ok(job.as_ref().map(JobSnapshot::from_job)),
                Err(e) => Err(ApiError::from_kube("get job", e)),
            }
        })
    }

    fn list_events(&self, about: &str) -> BoxFuture<'_, Result<Vec<EventSnapshot>, ApiError>> {
        let field = format!("involvedObject.name={about}");
        Box::pin(async move {
            let params = kube::api::ListParams::default().fields(&field);
            let listed = self
                .events
                .list(&params)
                .await
                .map_err(|e| ApiError::from_kube("list events", e))?;
            Ok(listed.items.iter().map(EventSnapshot::from_event).collect())
        })
    }

    fn list(&self, selector: &Selector) -> BoxFuture<'_, Result<Listing, ApiError>> {
        let selector = selector.0.clone();
        Box::pin(async move {
            let params = kube::api::ListParams::default().labels(&selector);
            let jobs = self
                .jobs
                .list(&params)
                .await
                .map_err(|e| ApiError::from_kube("list jobs", e))?;
            let pods = self
                .pods
                .list(&params)
                .await
                .map_err(|e| ApiError::from_kube("list pods", e))?;
            Ok(Listing {
                jobs: jobs.items.iter().map(JobSnapshot::from_job).collect(),
                pods: pods.items.iter().map(PodSnapshot::from_pod).collect(),
            })
        })
    }

    fn patch_labels(
        &self,
        kind: ObjectKind,
        name: &str,
        labels: &[(&str, Option<&str>)],
    ) -> BoxFuture<'_, Result<(), ApiError>> {
        let name = name.to_string();
        let mut map = serde_json::Map::new();
        for (k, v) in labels {
            map.insert(
                (*k).to_string(),
                v.map_or(serde_json::Value::Null, |s| {
                    serde_json::Value::String(s.to_string())
                }),
            );
        }
        let patch = serde_json::json!({ "metadata": { "labels": map } });
        Box::pin(async move {
            let params = kube::api::PatchParams::default();
            let merge = kube::api::Patch::Merge(&patch);
            match kind {
                ObjectKind::Job => self
                    .jobs
                    .patch(&name, &params, &merge)
                    .await
                    .map(|_| ())
                    .map_err(|e| ApiError::from_kube("patch job labels", e)),
                ObjectKind::Pod => self
                    .pods
                    .patch(&name, &params, &merge)
                    .await
                    .map(|_| ())
                    .map_err(|e| ApiError::from_kube("patch pod labels", e)),
            }
        })
    }

    fn delete_job(&self, name: &str) -> BoxFuture<'_, Result<(), ApiError>> {
        let name = name.to_string();
        Box::pin(async move {
            let params = kube::api::DeleteParams::foreground();
            match self.jobs.delete(&name, &params).await {
                Ok(_) => Ok(()),
                Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
                Err(e) => Err(ApiError::from_kube("delete job", e)),
            }
        })
    }

    fn watch(&self, selector: &Selector) -> BoxStream<'_, WatchFrame> {
        use futures::StreamExt;
        use kube::runtime::watcher;
        let config = watcher::Config::default().labels(&selector.0);
        let jobs = watcher(self.jobs.clone(), config.clone()).map(|ev| match ev {
            Ok(watcher::Event::Apply(job)) => Some(WatchFrame::Job(JobSnapshot::from_job(&job))),
            Ok(watcher::Event::Delete(job)) => {
                Some(WatchFrame::JobDeleted(JobSnapshot::from_job(&job)))
            }
            Ok(watcher::Event::Init | watcher::Event::InitDone) => Some(WatchFrame::Resync),
            Ok(watcher::Event::InitApply(job)) => {
                Some(WatchFrame::Job(JobSnapshot::from_job(&job)))
            }
            Err(_) => None,
        });
        let pods = watcher(self.pods.clone(), config).map(|ev| match ev {
            Ok(watcher::Event::Apply(pod)) => Some(WatchFrame::Pod(PodSnapshot::from_pod(&pod))),
            Ok(watcher::Event::Delete(pod)) => {
                Some(WatchFrame::PodDeleted(PodSnapshot::from_pod(&pod)))
            }
            Ok(watcher::Event::Init | watcher::Event::InitDone) => Some(WatchFrame::Resync),
            Ok(watcher::Event::InitApply(pod)) => {
                Some(WatchFrame::Pod(PodSnapshot::from_pod(&pod)))
            }
            Err(_) => None,
        });
        Box::pin(futures::stream::select(jobs, pods).filter_map(|f| async move { f }))
    }
}
