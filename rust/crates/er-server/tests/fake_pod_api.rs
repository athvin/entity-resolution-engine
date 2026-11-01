//! A test double of the KUBERNETES API, not a second executor (§6.9): on
//! `create_job` it runs the real `er-server exec-task` binary as a child
//! process with the manifest's argv and env, and synthesizes the WatchFrames a
//! cluster would emit — including scripted failures (OOMKilled) for the
//! classifier path. It has no scheduling, retry, readiness or classification;
//! all of that is the real dispatcher code against these frames.
//! `no_second_executor.rs` keeps this type out of the binary.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use futures::future::BoxFuture;
use futures::stream::BoxStream;

use er_k8s::k8s_openapi::api::batch::v1::Job;
use er_k8s::{
    ApiError, EventSnapshot, JobSnapshot, Listing, ObjectKind, PodApi, PodPhase, PodSnapshot,
    Selector, WatchFrame,
};

/// What a scripted attempt does instead of running the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// Emit Running then Failed/OOMKilled, run nothing.
    OomKill,
}

pub struct FakePodApi {
    /// Where this double's "cluster" reaches the control plane.
    pub control_url: String,
    /// Where termination records land, per attempt.
    pub scratch: std::path::PathBuf,
    jobs: Mutex<BTreeMap<String, JobSnapshot>>,
    frames: tokio::sync::mpsc::UnboundedSender<WatchFrame>,
    /// task_instance_id → script override.
    #[expect(
        clippy::zero_sized_map_values,
        reason = "Script is a one-variant enum today; the map is its shape as failure modes grow"
    )]
    scripts: Mutex<BTreeMap<String, Script>>,
    counter: std::sync::atomic::AtomicU64,
}

impl FakePodApi {
    #[expect(
        clippy::zero_sized_map_values,
        reason = "Script is a one-variant enum today; the map is its shape as failure modes grow"
    )]
    pub fn new(
        control_url: String,
        scratch: std::path::PathBuf,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<WatchFrame>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            FakePodApi {
                control_url,
                scratch,
                jobs: Mutex::new(BTreeMap::new()),
                frames: tx,
                scripts: Mutex::new(BTreeMap::new()),
                counter: std::sync::atomic::AtomicU64::new(0),
            },
            rx,
        )
    }

    pub fn script(&self, task_instance_id: &str, script: Script) {
        self.scripts
            .lock()
            .unwrap()
            .insert(task_instance_id.to_string(), script);
    }

    pub fn unscript(&self, task_instance_id: &str) {
        self.scripts.lock().unwrap().remove(task_instance_id);
    }

    fn pod_base(job: &JobSnapshot, uid: &str) -> PodSnapshot {
        PodSnapshot {
            name: format!("{}-pod", job.name),
            uid: uid.to_string(),
            node_name: Some("fake-node".into()),
            labels: job.labels.clone(),
            annotations: job.annotations.clone(),
            phase: PodPhase::Pending,
            waiting_reason: None,
            terminated_exit_code: None,
            terminated_reason: None,
            termination_message: None,
            disruption_target: None,
            status_reason: None,
            unschedulable: false,
            scheduling_message: None,
            node_not_ready: false,
        }
    }
}

impl PodApi for FakePodApi {
    fn create_job(&self, job: &Job) -> BoxFuture<'_, Result<(), ApiError>> {
        let snapshot = JobSnapshot::from_job(job);
        let name = snapshot.name.clone();
        {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.contains_key(&name) {
                return Box::pin(async { Err(ApiError::AlreadyExists) });
            }
            jobs.insert(name.clone(), snapshot.clone());
        }
        let attempt = snapshot
            .annotations
            .get(er_k8s::manifest::TASK_INSTANCE_ANNOTATION)
            .cloned()
            .unwrap_or_default();
        let script = self.scripts.lock().unwrap().get(&attempt).copied();
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let uid = format!("fake-uid-{n}");
        let frames = self.frames.clone();
        let mut pod = Self::pod_base(&snapshot, &uid);
        let control_url = self.control_url.clone();
        let termination = self.scratch.join(format!("term-{attempt}-{n}.json"));

        // The container's argv and env, exactly as the manifest carries them.
        let (args, mut env): (Vec<String>, Vec<(String, String)>) = job
            .spec
            .as_ref()
            .and_then(|s| s.template.spec.as_ref())
            .and_then(|p| p.containers.first())
            .map(|c| {
                (
                    c.args.clone().unwrap_or_default(),
                    c.env
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|e| e.value.map(|v| (e.name, v)))
                        .collect(),
                )
            })
            .unwrap_or_default();
        env.push(("ER_CONTROL_URL".into(), control_url));
        env.push((
            "ER_TERMINATION_LOG".into(),
            termination.display().to_string(),
        ));
        env.push(("ER_POD_UID".into(), uid.clone()));
        env.push(("ER_POD_NAME".into(), pod.name.clone()));

        Box::pin(async move {
            match script {
                Some(Script::OomKill) => {
                    pod.phase = PodPhase::Running;
                    let _ = frames.send(WatchFrame::Pod(pod.clone()));
                    pod.phase = PodPhase::Failed;
                    pod.terminated_exit_code = Some(137);
                    pod.terminated_reason = Some("OOMKilled".into());
                    let _ = frames.send(WatchFrame::Pod(pod));
                }
                None => {
                    // The real verb, as a child process — the cluster's half is
                    // exactly "run the image's command and surface its exit".
                    tokio::task::spawn_blocking(move || {
                        pod.phase = PodPhase::Running;
                        let _ = frames.send(WatchFrame::Pod(pod.clone()));
                        let mut command =
                            std::process::Command::new(env!("CARGO_BIN_EXE_er-server"));
                        command.arg("exec-task").args(&args);
                        for (k, v) in env {
                            command.env(k, v);
                        }
                        let status = command.status();
                        let exit = status.ok().and_then(|s| s.code()).unwrap_or(1);
                        // Faithful to the kubelet: ANY non-zero exit is a
                        // Failed phase — including exit 10 (`nothing_to_do`),
                        // whose success the termination record carries.
                        pod.phase = if exit == 0 {
                            PodPhase::Succeeded
                        } else {
                            PodPhase::Failed
                        };
                        pod.terminated_exit_code = Some(exit);
                        pod.termination_message = std::fs::read_to_string(&termination).ok();
                        let _ = frames.send(WatchFrame::Pod(pod));
                    });
                }
            }
            Ok(())
        })
    }

    fn get_job(&self, name: &str) -> BoxFuture<'_, Result<Option<JobSnapshot>, ApiError>> {
        let found = self.jobs.lock().unwrap().get(name).cloned();
        Box::pin(async move { Ok(found) })
    }

    fn list_events(&self, _about: &str) -> BoxFuture<'_, Result<Vec<EventSnapshot>, ApiError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn list(&self, _selector: &Selector) -> BoxFuture<'_, Result<Listing, ApiError>> {
        let jobs: Vec<JobSnapshot> = self.jobs.lock().unwrap().values().cloned().collect();
        Box::pin(async move {
            Ok(Listing {
                jobs,
                pods: Vec::new(),
            })
        })
    }

    fn patch_labels(
        &self,
        kind: ObjectKind,
        name: &str,
        labels: &[(&str, Option<&str>)],
    ) -> BoxFuture<'_, Result<(), ApiError>> {
        if kind == ObjectKind::Job
            && let Some(job) = self.jobs.lock().unwrap().get_mut(name)
        {
            for (k, v) in labels {
                match v {
                    Some(v) => {
                        job.labels.insert((*k).to_string(), (*v).to_string());
                    }
                    None => {
                        job.labels.remove(*k);
                    }
                }
            }
        }
        Box::pin(async { Ok(()) })
    }

    fn delete_job(&self, name: &str) -> BoxFuture<'_, Result<(), ApiError>> {
        self.jobs.lock().unwrap().remove(name);
        Box::pin(async { Ok(()) })
    }

    fn watch(&self, _selector: &Selector) -> BoxStream<'_, WatchFrame> {
        Box::pin(futures::stream::pending())
    }
}
