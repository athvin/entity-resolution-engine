"""The Kubernetes Job launcher: one batch/v1 Job per run, behind the ``launch`` seam.

docs/infrastructure.md §6: a pipeline run is one ephemeral pod, scoped to one
tenant, that exits when the run finishes. This module substitutes a Job for the
subprocess the single-VM dispatcher forks, conforming to the same
:data:`~erserver.dispatcher.LaunchFn` seam — ``run_once`` cannot tell them apart.

What the Job spec has to get right (§6.2), in order of how expensively each
fails:

* ``backoffLimit: 0`` — the engine owns retries, not Kubernetes. The default of
  6 would re-run a config error six times and a partially-completed pipeline
  without ``--resume``.
* The EFS mount — the runner's first act is opening the org config by path, and
  imports list CSV drops from ``drop_root``; both live on the shared filesystem.
* Explicit ``requests.ephemeral-storage`` and per-volume ``sizeLimit`` on the
  two DuckDB spill emptyDirs (§5.3) — without them one runaway run evicts every
  pod on the node, or the runner evicts itself at the same point three times.
* ``karpenter.sh/do-not-disrupt`` — §5.1: consolidation must not discard an
  hour of a 10M-row run to bin-pack onto a cheaper node.
* ``ER_DUCKDB_THREADS``/``ER_DUCKDB_MEMORY_LIMIT`` set in the same operation as
  the pod resources, from the §6.5 class riding the job row — DuckDB reads
  neither cgroup limits nor host core count (§2).

Progress is NOT this module's job (gate D3): the runner writes its own stage
rows to Postgres (``run_stages``, via :mod:`er.obs.runctx`) as each stage
completes, so live progress survives a dispatcher restart with no pod-log
stream anywhere. The one log read here is a single tail fetch *after* the Job
reaches a terminal state, to recover the runner's terminal result line — the
exit-code taxonomy's ``error_class`` — and it degrades gracefully: an
unreadable log still yields the container exit code, which drives
:func:`erserver.policy.dispose` for every terminal row of the matrix.

Secrets keep §9's shape: the control plane stores ``secret://`` references and
the dispatcher resolves them at dispatch, exactly as in subprocess mode. The
pod's baseline secret env (``ERSERVER_SECRET_*``, DSNs) arrives via ``envFrom``
from the same ExternalSecret-projected Secret the dispatcher itself reads —
those values never ride the Job manifest.
"""

from __future__ import annotations

import json
import re
import time
from collections.abc import Callable
from typing import Any

from kubernetes import client as k8s_client
from kubernetes import config as k8s_config
from kubernetes.client.exceptions import ApiException

from erserver import sizing
from erserver.dispatcher import RunnerResult
from erserver.settings import ServerSettings

__all__ = [
    "KubernetesLauncher",
    "RUNNER_TAINT_KEY",
    "build_job_manifest",
    "job_name",
    "pod_exit_state",
    "runner_env",
]

#: The taint both runner NodePools carry (infra/k8s/nodepools/runner-*.yaml):
#: only runner Jobs, which tolerate it, land on the tainted NVMe capacity.
RUNNER_TAINT_KEY = "er.dupezero.com/runner"

#: Node label Karpenter stamps on every node it provisions; the §6.5 class
#: names which pool (``runner-sm``/``runner-l``) a run schedules onto.
NODEPOOL_LABEL = "karpenter.sh/nodepool"

#: §6.2: the dispatcher keeps the finished Job around long enough to read its
#: final status (and the terminal log line) even across its own restart.
TTL_SECONDS_AFTER_FINISHED = 3600

#: §6.2: must exceed the engine's stage-boundary check interval and the
#: dispatcher's own TERMINATE_GRACE_SECONDS (10s) so SIGTERM lets the engine
#: stop at a stage boundary instead of being SIGKILLed mid-write.
TERMINATION_GRACE_SECONDS = 30

#: How many trailing log lines to fetch once at termination. The terminal
#: result line and the last stage records sit at the very end of the stream;
#: the budget only needs to outreach dbt's final chatter.
LOG_TAIL_LINES = 2000

#: §9 keys the ExternalSecret-projected ``er-erserver-env`` Secret delivers to
#: the pod via ``envFrom``; they must not be duplicated into the Job manifest.
_SECRET_BACKED_KEYS = frozenset(
    {
        "ERSERVER_DSN",
        "ERSERVER_MAINT_DSN",
        "ERSERVER_TENANT_DSN_TEMPLATE",
        "ERSERVER_OPERATOR_TOKEN",
        "ERSERVER_SMTP_URL",
        "ERSERVER_TENANT_ENV_JSON",
    }
)

_NAME_SAFE = re.compile(r"[^a-z0-9-]")

#: Where the ServiceAccount admission controller mounts the pod's namespace.
_NAMESPACE_FILE = "/var/run/secrets/kubernetes.io/serviceaccount/namespace"


def job_name(job_id: str) -> str:
    """The deterministic Job name for a queue job: ``er-run-{job_id}``.

    Deterministic on purpose: a restarted dispatcher reconciles running job
    rows against existing Jobs by recomputing this name, and a crashed launch
    retried against an existing Job collides (409) instead of double-running.
    ULIDs are Crockford base32, so lowercasing yields a valid DNS-1123 label;
    anything else is defensively squashed.
    """
    return f"er-run-{_NAME_SAFE.sub('-', job_id.lower())}"[:63].rstrip("-")


def _carried(key: str) -> bool:
    """Whether one env entry rides the Job manifest (vs ``envFrom`` or nowhere).

    The seam hands over the dispatcher's whole merged environment; the pod gets
    only the engine/control-plane variables. Secret-backed baseline keys arrive
    via ``envFrom`` from the projected Secret instead, so no credential the
    dispatcher resolved from its own baseline is written into the Job object.
    """
    if key == "TRACEPARENT":
        return True
    if key.startswith("ERSERVER_SECRET_") or key in _SECRET_BACKED_KEYS:
        return False
    return key.startswith(("ER_", "ERSERVER_"))


def runner_env(env: dict[str, str], cls: sizing.ResourceClass) -> list[dict[str, str]]:
    """The pod's inline env: the filtered per-run overlay plus the class knobs.

    ``ER_DUCKDB_THREADS`` and ``ER_DUCKDB_MEMORY_LIMIT`` come from the §6.5
    class riding the job row, overriding any org-env value — per-run sizing
    lives on the job row, never ``orgs.env`` (§18.10), and the thread count
    must match the CPU request set in the same manifest (§6.5).
    """
    merged = {key: value for key, value in env.items() if _carried(key)}
    merged["ER_DUCKDB_THREADS"] = str(cls.duckdb_threads)
    merged["ER_DUCKDB_MEMORY_LIMIT"] = cls.duckdb_memory_limit
    return [{"name": key, "value": merged[key]} for key in sorted(merged)]


def build_job_manifest(
    *,
    name: str,
    namespace: str,
    image: str,
    payload: dict[str, Any],
    env: dict[str, str],
    cls: sizing.ResourceClass,
    efs_claim: str,
    efs_mount_path: str,
    env_secret: str | None,
    service_account: str | None,
) -> dict[str, Any]:
    """The batch/v1 Job for one run — §6.2's spec, field for field."""
    labels = {
        "app.kubernetes.io/name": "er-runner",
        "app.kubernetes.io/part-of": "er",
        "er.dupezero.com/job-id": _NAME_SAFE.sub("-", str(payload.get("job_id", "")).lower()),
        "er.dupezero.com/kind": str(payload.get("kind", "")),
        "er.dupezero.com/resource-class": cls.name,
    }
    container: dict[str, Any] = {
        "name": "runner",
        "image": image,
        "command": [
            "python",
            "-m",
            "erserver.runner",
            json.dumps(payload, separators=(",", ":")),
        ],
        "env": runner_env(env, cls),
        "resources": {
            "requests": {
                "cpu": cls.cpu,
                "memory": cls.memory,
                "ephemeral-storage": cls.ephemeral_storage,
            },
            # Limits == requests across the board. The dev namespace's
            # LimitRange would otherwise default-fill a missing cpu limit
            # BELOW the request and reject the pod; and §5.3's eviction
            # accounting needs memory/ephemeral-storage to be exact.
            "limits": {
                "cpu": cls.cpu,
                "memory": cls.memory,
                "ephemeral-storage": cls.ephemeral_storage,
            },
        },
        "volumeMounts": [
            # DuckDB's two CWD-relative spill directories (§5.3) — emptyDir on
            # the node's RAID0 instance-store NVMe, never the root filesystem.
            {"name": "spill", "mountPath": "/app/.tmp"},
            {"name": "dbt-spill", "mountPath": "/app/dbt/.tmp"},
            # §6.2: the Job cannot run without the shared filesystem — the
            # runner opens the org config and lists CSV drops by path.
            {"name": "er-efs", "mountPath": efs_mount_path},
        ],
        "securityContext": {
            "allowPrivilegeEscalation": False,
            "capabilities": {"drop": ["ALL"]},
        },
    }
    if env_secret:
        container["envFrom"] = [{"secretRef": {"name": env_secret}}]
    pod_spec: dict[str, Any] = {
        # §6.2: the engine owns retries; a dead pod is policy.dispose's row,
        # never the kubelet's.
        "restartPolicy": "Never",
        # The runner never talks to the Kubernetes API (§6.4's containment is
        # env + ephemerality + the advisory lock, not RBAC).
        "automountServiceAccountToken": False,
        "nodeSelector": {NODEPOOL_LABEL: cls.nodepool},
        "tolerations": [
            {
                "key": RUNNER_TAINT_KEY,
                "operator": "Equal",
                "value": "true",
                "effect": "NoSchedule",
            }
        ],
        "terminationGracePeriodSeconds": TERMINATION_GRACE_SECONDS,
        "securityContext": {
            "runAsNonRoot": True,
            "runAsUser": 1001,
            "runAsGroup": 1001,
            "fsGroup": 1001,
            "seccompProfile": {"type": "RuntimeDefault"},
        },
        "containers": [container],
        "volumes": [
            {"name": "spill", "emptyDir": {"sizeLimit": cls.ephemeral_storage}},
            {"name": "dbt-spill", "emptyDir": {"sizeLimit": cls.ephemeral_storage}},
            {"name": "er-efs", "persistentVolumeClaim": {"claimName": efs_claim}},
        ],
    }
    if service_account:
        # §6.4 anticipates a per-tenant value here (Phase 4); one shared
        # ServiceAccount until then.
        pod_spec["serviceAccountName"] = service_account
    return {
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {"name": name, "namespace": namespace, "labels": labels},
        "spec": {
            "backoffLimit": 0,
            "ttlSecondsAfterFinished": TTL_SECONDS_AFTER_FINISHED,
            "activeDeadlineSeconds": cls.active_deadline_seconds,
            "template": {
                "metadata": {
                    "labels": labels,
                    # §5.1: block voluntary disruption for the life of the run;
                    # the node still empties ~30s after the pod exits.
                    "annotations": {"karpenter.sh/do-not-disrupt": "true"},
                },
                "spec": pod_spec,
            },
        },
    }


def pod_exit_state(pod: Any, *, job_succeeded: bool = False) -> tuple[int, str]:
    """Map a terminal pod to the seam's returncode convention.

    Non-negative is the container's own exit status — the engine's S4.0
    taxonomy, handed to :func:`erserver.policy.dispose` untouched. Negative is
    "the process died without a status of its own" (OOM-kill, eviction, node
    loss, SIGKILL), which ``run_once`` turns into ``exit_code None`` — the
    infra row: requeue with ``--resume``, bounded by ``max_attempts``.
    """
    if pod is None:
        if job_succeeded:
            # Succeeded Job whose pod was already collected: trust the Job.
            return 0, "runner pod already collected; Job reported success"
        return -1, "runner pod not found: deleted, evicted, or its node was lost"
    status = getattr(pod, "status", None)
    for container_status in getattr(status, "container_statuses", None) or []:
        state = getattr(container_status, "state", None)
        terminated = getattr(state, "terminated", None)
        if terminated is None:
            continue
        reason = getattr(terminated, "reason", None)
        code = getattr(terminated, "exit_code", None)
        if reason == "OOMKilled":
            return -9, "runner OOM-killed (pod memory limit)"
        if code is None:
            return -1, "runner container terminated without an exit code"
        if int(code) >= 128:
            signal = int(code) - 128
            return -signal, f"runner killed by signal {signal} ({reason or 'Signaled'})"
        return int(code), f"runner exited {int(code)}"
    if getattr(status, "reason", None) == "Evicted":
        message = getattr(status, "message", None) or "no message"
        return -1, f"runner pod evicted: {message}"
    return -1, "runner pod reached a terminal phase with no terminated container state"


def _condition_true(status: Any, condition_type: str) -> bool:
    for condition in getattr(status, "conditions", None) or []:
        if (
            getattr(condition, "type", None) == condition_type
            and str(getattr(condition, "status", "")).lower() == "true"
        ):
            return True
    return False


class KubernetesLauncher:
    """A :data:`~erserver.dispatcher.LaunchFn` that runs each job as a k8s Job.

    Construction is cheap and side-effect free; API clients are built lazily on
    first use (in-cluster config first, kubeconfig as the operator fallback).
    Tests inject fakes via ``batch``/``core``.
    """

    def __init__(
        self,
        settings: ServerSettings,
        *,
        batch: Any | None = None,
        core: Any | None = None,
    ) -> None:
        if not settings.runner_image:
            raise ValueError(
                "ERSERVER_RUNNER_IMAGE is required when ERSERVER_LAUNCHER=kubernetes; "
                "an unpinned runner image is the §14.2 stale-code hazard"
            )
        self._settings = settings
        self._batch = batch
        self._core = core
        self._namespace: str | None = settings.k8s_namespace

    # -- plumbing ----------------------------------------------------------

    @property
    def namespace(self) -> str:
        if self._namespace is None:
            try:
                with open(_NAMESPACE_FILE, encoding="utf-8") as handle:
                    self._namespace = handle.read().strip() or "default"
            except OSError:
                self._namespace = "default"
        return self._namespace

    def _apis(self) -> tuple[Any, Any]:
        if self._batch is None or self._core is None:
            try:
                k8s_config.load_incluster_config()
            except k8s_config.ConfigException:
                k8s_config.load_kube_config()
            self._batch = k8s_client.BatchV1Api()
            self._core = k8s_client.CoreV1Api()
        return self._batch, self._core

    # -- the seam ----------------------------------------------------------

    def __call__(
        self,
        payload: dict[str, Any],
        env: dict[str, str],
        *,
        on_stage: Callable[[dict[str, Any]], None] | None = None,
        should_cancel: Callable[[], bool] | None = None,
    ) -> RunnerResult:
        """Create the run's Job and block until it reaches a terminal state."""
        batch, _ = self._apis()
        settings = self._settings
        cls = sizing.class_named(str(payload.get("resource_class", ""))) or sizing.DEFAULT_CLASS
        name = job_name(str(payload["job_id"]))
        manifest = build_job_manifest(
            name=name,
            namespace=self.namespace,
            image=settings.runner_image or "",
            payload=payload,
            env=env,
            cls=cls,
            efs_claim=settings.runner_efs_claim,
            efs_mount_path=settings.runner_efs_mount_path,
            env_secret=settings.runner_env_secret,
            service_account=settings.runner_service_account,
        )
        try:
            batch.create_namespaced_job(namespace=self.namespace, body=manifest)
        except ApiException as exc:
            if exc.status != 409:
                raise
            # Already exists: a prior dispatcher died between create and wait.
            # The deterministic name makes the retry an adoption, not a twin.
        return self._wait(name, on_stage=on_stage, should_cancel=should_cancel)

    def exists(self, job_id: str) -> bool:
        """Whether this queue job's runner Job exists, by deterministic name."""
        batch, _ = self._apis()
        try:
            batch.read_namespaced_job(name=job_name(job_id), namespace=self.namespace)
        except ApiException as exc:
            if exc.status == 404:
                return False
            raise
        return True

    def adopt(
        self,
        job_id: str,
        *,
        on_stage: Callable[[dict[str, Any]], None] | None = None,
        should_cancel: Callable[[], bool] | None = None,
    ) -> RunnerResult:
        """Wait out an already-running Job — the restart-survival half of §6.

        The run kept going while the dispatcher was down (the runner owns its
        own progress, D3); the restarted dispatcher only has to pick the
        completion back up.
        """
        self._apis()
        return self._wait(job_name(job_id), on_stage=on_stage, should_cancel=should_cancel)

    # -- waiting and terminal mapping ---------------------------------------

    def _wait(
        self,
        name: str,
        *,
        on_stage: Callable[[dict[str, Any]], None] | None,
        should_cancel: Callable[[], bool] | None,
    ) -> RunnerResult:
        batch, _ = self._apis()
        canceled = False
        while True:
            try:
                job = batch.read_namespaced_job(name=name, namespace=self.namespace)
            except ApiException as exc:
                if exc.status != 404:
                    raise
                # Deleted: our own cancellation, or an operator's. Either way
                # the run is resumable — the in-lake ledger knows its stages.
                return RunnerResult(
                    returncode=-15 if canceled else -1,
                    stdout="",
                    stderr=f"runner Job {name} no longer exists",
                    canceled=canceled,
                )
            status = getattr(job, "status", None)
            succeeded = bool(getattr(status, "succeeded", None)) or _condition_true(
                status, "Complete"
            )
            failed = bool(getattr(status, "failed", None)) or _condition_true(status, "Failed")
            if succeeded or failed:
                return self._terminal_result(
                    name, job_succeeded=succeeded, canceled=canceled, on_stage=on_stage
                )
            if not canceled and should_cancel is not None and should_cancel():
                canceled = True
                self._delete(name)
            time.sleep(self._settings.k8s_poll_seconds)

    def _delete(self, name: str) -> None:
        batch, _ = self._apis()
        try:
            batch.delete_namespaced_job(
                name=name,
                namespace=self.namespace,
                propagation_policy="Foreground",
                grace_period_seconds=TERMINATION_GRACE_SECONDS,
            )
        except ApiException as exc:
            if exc.status != 404:
                raise

    def _terminal_result(
        self,
        name: str,
        *,
        job_succeeded: bool,
        canceled: bool,
        on_stage: Callable[[dict[str, Any]], None] | None,
    ) -> RunnerResult:
        pod = self._runner_pod(name)
        returncode, detail = pod_exit_state(pod, job_succeeded=job_succeeded)
        log_tail = self._log_tail(pod)
        if on_stage is not None:
            for record in _stage_records(log_tail):
                on_stage(record)
        return RunnerResult(
            returncode=returncode,
            stdout=log_tail,
            stderr=log_tail if log_tail else detail,
            canceled=canceled,
        )

    def _runner_pod(self, name: str) -> Any | None:
        _, core = self._apis()
        try:
            pods = core.list_namespaced_pod(
                namespace=self.namespace, label_selector=f"job-name={name}"
            )
        except ApiException:
            return None
        items = list(getattr(pods, "items", None) or [])
        if not items:
            return None

        # backoffLimit 0 means at most one pod ever; take the newest anyway.
        def _created(pod: Any) -> str:
            metadata = getattr(pod, "metadata", None)
            return str(getattr(metadata, "creation_timestamp", "") or "")

        return sorted(items, key=_created)[-1]

    def _log_tail(self, pod: Any | None) -> str:
        """One tail fetch after termination — never a stream, never progress.

        Recovers the runner's terminal result line (and the trailing S5.2 stage
        records) from the finished pod. Best-effort by design: on any failure
        the container exit code alone still drives the retry matrix.
        """
        if pod is None:
            return ""
        metadata = getattr(pod, "metadata", None)
        pod_name = getattr(metadata, "name", None)
        if not pod_name:
            return ""
        _, core = self._apis()
        try:
            text = core.read_namespaced_pod_log(
                name=pod_name, namespace=self.namespace, tail_lines=LOG_TAIL_LINES
            )
        except ApiException:
            return ""
        return str(text) if text is not None else ""


def _stage_records(log_tail: str) -> list[dict[str, Any]]:
    """The S5.2 stage records present in the tail, oldest first.

    The same shape the subprocess launcher streams from stderr; replaying them
    through ``on_stage`` at completion keeps ``jobs.progress`` and the
    ``review.created`` roll-up populated without any mid-run log stream.
    """
    records: list[dict[str, Any]] = []
    for line in log_tail.splitlines():
        stripped = line.strip()
        if not stripped.startswith("{"):
            continue
        try:
            record = json.loads(stripped)
        except json.JSONDecodeError:
            continue
        # The same criterion the subprocess stderr reader applies: a JSON dict
        # carrying "stage". The terminal result line has no "stage" key.
        if isinstance(record, dict) and "stage" in record:
            records.append(record)
    return records
