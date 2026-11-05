//! The per-attempt Job manifest (§6.2/§6.8). The template is YAML the chart
//! owns (Helm fills the cluster-level values at install; the dispatcher reads
//! the rendered template from a mounted file, never through the API) with
//! `${ER_…}` placeholders for the per-attempt fields this module substitutes.
//! The result parses into a typed `batch/v1 Job` — a template that renders
//! unparseable YAML is a launch-time refusal, not a cluster-side surprise.

use k8s_openapi::api::batch::v1::Job;

/// The per-attempt fields the launcher substitutes into the chart's template.
#[derive(Debug, Clone)]
pub struct RenderAttempt {
    /// `er-t-<20 hex of task_instances.id>-l<launches>` — a pure function of
    /// the attempt key and launch count (§4.2), so a retried create is
    /// idempotent on the name.
    pub job_name: String,
    pub task_instance_id: String,
    pub task_id: String,
    pub job_id: String,
    pub try_number: i64,
    pub resource_class: String,
    pub owner_epoch: String,
    pub structural_fingerprint: String,
    pub policy_hash: String,
    /// The segment's summed timeouts, or the class default (§6.2).
    pub active_deadline_seconds: i64,
    /// The pod's argv tail: the last task of its fused segment.
    pub through_task: String,
    /// The image digest this attempt runs, recorded as an annotation so an
    /// in-flight pod says which release launched it.
    ///
    /// §6.2's Job spec lists it under `annotations` — the "authority" block —
    /// and it HAS to be an annotation rather than a label: `sha256:` plus 64
    /// hex is 71 characters and a label VALUE caps at 63, so the obvious
    /// reading of "add the image-digest label" produces an object the API
    /// server rejects.
    ///
    /// Not an adoption input. §6.2 is explicit that "an image-digest-only
    /// difference is adopted because the contract is the fingerprint". This
    /// exists to make a cutover legible — which colour's image each running
    /// attempt came from — not to decide anything.
    ///
    /// Empty when the deployment pins a TAG rather than a digest: the chart
    /// only sets `ER_IMAGE_DIGEST` `{{- with .Values.images.api.digest }}`, so
    /// kind and a tag-based dev install have none. An empty annotation value is
    /// legal and reads correctly as "not deployed by digest".
    pub image_digest: String,
}

/// Why a render was refused.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("the task template leaves `{placeholder}` unsubstituted")]
    Unsubstituted { placeholder: String },
    #[error("the rendered task template is not a batch/v1 Job")]
    Parse(#[source] serde_norway::Error),
}

/// The §4.2 Job name: 20 hex of the attempt id plus the launch count.
#[must_use]
pub fn job_name(task_instance_id: &str, launches: i64) -> String {
    let hex: String = task_instance_id.chars().take(20).collect();
    format!("er-t-{hex}-l{launches}")
}

/// Substitute the per-attempt placeholders and parse the result.
///
/// # Errors
/// A placeholder this attempt does not fill survived substitution, or the
/// rendered YAML is not a Job.
pub fn render_job(template: &str, attempt: &RenderAttempt) -> Result<Job, RenderError> {
    let rendered = template
        .replace("${ER_JOB_NAME}", &attempt.job_name)
        .replace("${ER_TASK_INSTANCE_ID}", &attempt.task_instance_id)
        .replace("${ER_TASK_ID}", &attempt.task_id)
        .replace("${ER_JOB_ID}", &attempt.job_id)
        .replace("${ER_TRY_NUMBER}", &attempt.try_number.to_string())
        .replace("${ER_RESOURCE_CLASS}", &attempt.resource_class)
        .replace("${ER_OWNER_EPOCH}", &attempt.owner_epoch)
        .replace(
            "${ER_STRUCTURAL_FINGERPRINT}",
            &attempt.structural_fingerprint,
        )
        .replace("${ER_POLICY_HASH}", &attempt.policy_hash)
        .replace(
            "${ER_ACTIVE_DEADLINE_SECONDS}",
            &attempt.active_deadline_seconds.to_string(),
        )
        .replace("${ER_THROUGH_TASK}", &attempt.through_task)
        .replace("${ER_IMAGE_DIGEST}", &attempt.image_digest);
    if let Some(at) = rendered.find("${ER_") {
        let tail: String = rendered[at..]
            .chars()
            .take_while(|c| *c != '}')
            .chain(std::iter::once('}'))
            .collect();
        return Err(RenderError::Unsubstituted { placeholder: tail });
    }
    serde_norway::from_str(&rendered).map_err(RenderError::Parse)
}

/// The labels every managed object carries (selectors only, §6.2); the
/// annotations are authority and ride the template.
pub const MANAGED_LABEL: &str = "er.dupezero.com/managed";
pub const OWNER_LABEL: &str = "er.dupezero.com/owner";
pub const DONE_LABEL: &str = "er.dupezero.com/done";
pub const REVOKED: &str = "revoked";
pub const TASK_INSTANCE_ANNOTATION: &str = "er.dupezero.com/task-instance-id";
pub const FINGERPRINT_ANNOTATION: &str = "er.dupezero.com/structural-fingerprint";

/// A reference template with the §6.2 shape, for tests, kind, and the chart's
/// starting point. The chart's copy is the deployed authority.
pub const REFERENCE_TEMPLATE: &str = r#"
apiVersion: batch/v1
kind: Job
metadata:
  name: ${ER_JOB_NAME}
  labels:
    er.dupezero.com/managed: task
    er.dupezero.com/job: "${ER_JOB_ID}"
    er.dupezero.com/task: "${ER_TASK_ID}"
    er.dupezero.com/try: "${ER_TRY_NUMBER}"
    er.dupezero.com/class: "${ER_RESOURCE_CLASS}"
    er.dupezero.com/owner: "${ER_OWNER_EPOCH}"
  annotations:
    er.dupezero.com/task-instance-id: "${ER_TASK_INSTANCE_ID}"
    er.dupezero.com/task-id: "${ER_TASK_ID}"
    er.dupezero.com/structural-fingerprint: "${ER_STRUCTURAL_FINGERPRINT}"
    er.dupezero.com/policy-hash: "${ER_POLICY_HASH}"
    er.dupezero.com/image-digest: "${ER_IMAGE_DIGEST}"
spec:
  backoffLimit: 0
  activeDeadlineSeconds: ${ER_ACTIVE_DEADLINE_SECONDS}
  ttlSecondsAfterFinished: 300
  template:
    metadata:
      labels:
        er.dupezero.com/managed: task
        er.dupezero.com/job: "${ER_JOB_ID}"
        er.dupezero.com/task: "${ER_TASK_ID}"
        er.dupezero.com/try: "${ER_TRY_NUMBER}"
        er.dupezero.com/class: "${ER_RESOURCE_CLASS}"
        er.dupezero.com/owner: "${ER_OWNER_EPOCH}"
      annotations:
        karpenter.sh/do-not-disrupt: "true"
        er.dupezero.com/task-instance-id: "${ER_TASK_INSTANCE_ID}"
    spec:
      restartPolicy: Never
      terminationGracePeriodSeconds: 120
      containers:
        - name: task
          image: er-api
          command: ["er-server", "exec-task"]
          args:
            - "--attempt"
            - "${ER_TASK_INSTANCE_ID}"
            - "--expect-structural"
            - "${ER_STRUCTURAL_FINGERPRINT}"
            - "--through"
            - "${ER_THROUGH_TASK}"
          terminationMessagePath: /dev/termination-log
          terminationMessagePolicy: File
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt() -> RenderAttempt {
        RenderAttempt {
            job_name: job_name(&"ab".repeat(32), 1),
            task_instance_id: "ab".repeat(32),
            task_id: "maintain".into(),
            job_id: "job-1".into(),
            try_number: 1,
            resource_class: "M".into(),
            owner_epoch: "epoch-1".into(),
            structural_fingerprint: "ff".repeat(32),
            policy_hash: "ee".repeat(32),
            active_deadline_seconds: 21_600,
            through_task: "maintain".into(),
            image_digest: "sha256:feed".into(),
        }
    }

    #[test]
    fn the_reference_template_renders_a_typed_job() {
        let job = render_job(REFERENCE_TEMPLATE, &attempt()).expect("renders");
        assert_eq!(
            job.metadata.name.as_deref(),
            Some("er-t-abababababababababab-l1")
        );
        let spec = job.spec.expect("spec");
        assert_eq!(
            spec.backoff_limit,
            Some(0),
            "INVARIANT: Postgres owns retry"
        );
        assert_eq!(spec.active_deadline_seconds, Some(21_600));
        let pod = spec.template.spec.expect("pod spec");
        assert_eq!(pod.restart_policy.as_deref(), Some("Never"), "INVARIANT");
        let args = pod.containers[0].args.clone().unwrap();
        assert_eq!(args[1], "ab".repeat(32));
    }

    #[test]
    fn the_image_digest_lands_as_an_annotation_not_a_label() {
        // §6.2 lists it under annotations, and that is not a style choice:
        // `sha256:` plus 64 hex is 71 characters and a label VALUE caps at 63,
        // so the obvious reading — "add the image-digest label" — produces an
        // object the API server rejects.
        let mut a = attempt();
        a.image_digest = format!("sha256:{}", "ab".repeat(32));
        let job = render_job(REFERENCE_TEMPLATE, &a).expect("renders");
        let ann = job.metadata.annotations.clone().expect("annotations");
        assert_eq!(
            ann.get("er.dupezero.com/image-digest"),
            Some(&a.image_digest)
        );
        assert!(
            a.image_digest.len() > 63,
            "the length that forces the choice"
        );
        let labels = job.metadata.labels.clone().expect("labels");
        assert!(
            !labels.contains_key("er.dupezero.com/image-digest"),
            "a 71-character label value would be rejected by the API server"
        );
    }

    #[test]
    fn an_absent_digest_still_substitutes() {
        // The hazard this guards: a template placeholder that is not
        // substituted is a LAUNCH-TIME REFUSAL (`RenderError::Unsubstituted`),
        // so adding the annotation without handling a tag-based install would
        // have failed every launch on kind and on `make dev-up TAG=…`, where
        // the chart sets no ER_IMAGE_DIGEST at all.
        let mut a = attempt();
        a.image_digest = String::new();
        let job = render_job(REFERENCE_TEMPLATE, &a).expect("renders with no digest");
        let ann = job.metadata.annotations.clone().expect("annotations");
        assert_eq!(
            ann.get("er.dupezero.com/image-digest"),
            Some(&String::new()),
            "empty reads as 'not deployed by digest'; absent would read as a bug"
        );
    }

    #[test]
    fn an_unfilled_placeholder_is_a_refusal() {
        let err = render_job("metadata: { name: ${ER_NOT_A_FIELD} }", &attempt()).unwrap_err();
        assert!(matches!(err, RenderError::Unsubstituted { .. }), "{err}");
    }

    #[test]
    fn job_names_are_a_pure_function_of_the_attempt_key() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(job_name(id, 1), "er-t-0123456789abcdef0123-l1");
        assert_eq!(job_name(id, 2), "er-t-0123456789abcdef0123-l2");
    }
}
