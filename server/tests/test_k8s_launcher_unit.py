"""The Kubernetes Job launcher, against a faked API server — no cluster.

Covers what docs/infrastructure.md §6.2 says the Job spec has to get right
(``backoffLimit: 0``, the spill emptyDirs, do-not-disrupt, the EFS mount, the
taint/pool targeting), the exit-state mapping onto the engine's taxonomy
(including every killed-shape → negative → ``exit_code None`` path the §5
retry matrix keys on), cancellation-by-deletion, and the startup
reconciliation that re-adopts live runs instead of double-dispatching them.
"""

from __future__ import annotations

from types import SimpleNamespace
from typing import Any

import pytest
from erserver import dispatcher, queue
from erserver.dispatcher import RunnerResult
from erserver.k8s import (
    RUNNER_TAINT_KEY,
    KubernetesLauncher,
    build_job_manifest,
    job_name,
    pod_exit_state,
    runner_env,
)
from erserver.policy import FAILED, QUEUED, RUNNING
from erserver.settings import ServerSettings
from erserver.sizing import CLASS_L, CLASS_M, CLASS_S
from kubernetes.client.exceptions import ApiException

JOB_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHJ"
RUN_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHK"


def settings(**overrides: Any) -> ServerSettings:
    base: dict[str, Any] = {
        "dsn": "postgresql://unused",
        "launcher": "kubernetes",
        "runner_image": "registry.example/er-pipeline:abc123",
        "k8s_namespace": "alice",
        "runner_service_account": "er-runner",
        "k8s_poll_seconds": 0.0,
    }
    base.update(overrides)
    return ServerSettings(**base)


def payload(**overrides: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "job_id": JOB_ID,
        "kind": "run_all_full",
        "params": {},
        "config_path": "/srv/er/configs/acme.yaml",
        "run_id": RUN_ID,
        "resource_class": "M",
    }
    base.update(overrides)
    return base


def manifest(env: dict[str, str] | None = None, **kwargs: Any) -> dict[str, Any]:
    arguments: dict[str, Any] = {
        "name": job_name(JOB_ID),
        "namespace": "alice",
        "image": "registry.example/er-pipeline:abc123",
        "payload": payload(),
        "env": env or {},
        "cls": CLASS_M,
        "efs_claim": "er-efs",
        "efs_mount_path": "/srv/er",
        "env_secret": "er-erserver-env",
        "service_account": "er-runner",
    }
    arguments.update(kwargs)
    return build_job_manifest(**arguments)


# --- the deterministic name ------------------------------------------------


def test_job_name_is_deterministic_and_dns_safe() -> None:
    assert job_name(JOB_ID) == f"er-run-{JOB_ID.lower()}"
    assert job_name(JOB_ID) == job_name(JOB_ID)
    assert len(job_name(JOB_ID)) <= 63
    # Anything outside the DNS-1123 alphabet is squashed, never crashes.
    assert job_name("Weird_ID!") == "er-run-weird-id"


# --- the §6.2 Job spec ------------------------------------------------------


def test_backoff_limit_zero_and_restart_never() -> None:
    built = manifest()
    assert built["spec"]["backoffLimit"] == 0
    assert built["spec"]["template"]["spec"]["restartPolicy"] == "Never"


def test_do_not_disrupt_rides_the_pod_template() -> None:
    annotations = manifest()["spec"]["template"]["metadata"]["annotations"]
    assert annotations["karpenter.sh/do-not-disrupt"] == "true"


def test_both_spill_emptydirs_have_size_limits_and_the_right_paths() -> None:
    built = manifest()
    spec = built["spec"]["template"]["spec"]
    empty_dirs = {
        volume["name"]: volume["emptyDir"]["sizeLimit"]
        for volume in spec["volumes"]
        if "emptyDir" in volume
    }
    assert empty_dirs == {"spill": "150Gi", "dbt-spill": "150Gi"}
    mounts = {mount["name"]: mount["mountPath"] for mount in spec["containers"][0]["volumeMounts"]}
    assert mounts["spill"] == "/app/.tmp"
    assert mounts["dbt-spill"] == "/app/dbt/.tmp"


def test_resources_match_the_class_including_ephemeral_storage() -> None:
    for cls in (CLASS_S, CLASS_M, CLASS_L):
        resources = manifest(cls=cls)["spec"]["template"]["spec"]["containers"][0]["resources"]
        assert resources["requests"]["cpu"] == cls.cpu
        assert resources["requests"]["memory"] == cls.memory
        assert resources["requests"]["ephemeral-storage"] == cls.ephemeral_storage
        # Limits == requests: the dev namespace's LimitRange default-fills any
        # missing limit below the request and rejects the pod.
        assert resources["limits"] == resources["requests"]


def test_class_drives_deadline_pool_and_duckdb_env_together() -> None:
    built = manifest(cls=CLASS_L, env={"ER_DUCKDB_THREADS": "2", "ER_DUCKDB_MEMORY_LIMIT": "2GB"})
    assert built["spec"]["activeDeadlineSeconds"] == CLASS_L.active_deadline_seconds
    spec = built["spec"]["template"]["spec"]
    assert spec["nodeSelector"] == {"karpenter.sh/nodepool": "runner-l"}
    env = {entry["name"]: entry["value"] for entry in spec["containers"][0]["env"]}
    # §6.5: threads/limit come from the class riding the job row, in the same
    # operation as the pod resources — never from a stale orgs.env value.
    assert env["ER_DUCKDB_THREADS"] == str(CLASS_L.duckdb_threads)
    assert env["ER_DUCKDB_MEMORY_LIMIT"] == CLASS_L.duckdb_memory_limit


def test_tolerates_the_runner_taint() -> None:
    tolerations = manifest()["spec"]["template"]["spec"]["tolerations"]
    assert {
        "key": RUNNER_TAINT_KEY,
        "operator": "Equal",
        "value": "true",
        "effect": "NoSchedule",
    } in tolerations


def test_efs_claim_is_mounted_at_the_shared_path() -> None:
    spec = manifest()["spec"]["template"]["spec"]
    claims = [volume for volume in spec["volumes"] if "persistentVolumeClaim" in volume]
    assert claims == [{"name": "er-efs", "persistentVolumeClaim": {"claimName": "er-efs"}}]
    mounts = {mount["name"]: mount["mountPath"] for mount in spec["containers"][0]["volumeMounts"]}
    assert mounts["er-efs"] == "/srv/er"


def test_service_account_is_named_but_token_automount_stays_off() -> None:
    spec = manifest()["spec"]["template"]["spec"]
    assert spec["serviceAccountName"] == "er-runner"
    assert spec["automountServiceAccountToken"] is False


def test_secret_indirection_stays_intact() -> None:
    """§9: baseline secrets ride envFrom, never the manifest; the per-run
    overlay arrives exactly as the seam resolved it."""
    env = {
        # The dispatcher's own baseline — delivered to the pod via envFrom.
        "ERSERVER_SECRET_S3_KEY": "AKIA-REAL-VALUE",
        "ERSERVER_DSN": "postgresql://control-plane",
        "ERSERVER_MAINT_DSN": "postgresql://maint",
        # The per-run overlay, already resolved by resolve_env at dispatch.
        "ER_S3_ACCESS_KEY_ID": "AKIA-REAL-VALUE",
        "ER_CATALOG_DSN": "postgresql://tenant",
        # Non-secret server settings the runner (provision) reads.
        "ERSERVER_CONFIG_ROOT": "/srv/er/configs",
        # Dispatcher-pod noise that must not leak into the Job.
        "PATH": "/usr/bin",
        "KUBERNETES_SERVICE_HOST": "10.0.0.1",
        "TRACEPARENT": "00-abc-def-01",
    }
    container = manifest(env=env)["spec"]["template"]["spec"]["containers"][0]
    rendered = {entry["name"]: entry["value"] for entry in container["env"]}
    assert "ERSERVER_SECRET_S3_KEY" not in rendered
    assert "ERSERVER_DSN" not in rendered
    assert "ERSERVER_MAINT_DSN" not in rendered
    assert "PATH" not in rendered
    assert "KUBERNETES_SERVICE_HOST" not in rendered
    assert rendered["ER_S3_ACCESS_KEY_ID"] == "AKIA-REAL-VALUE"
    assert rendered["ER_CATALOG_DSN"] == "postgresql://tenant"
    assert rendered["ERSERVER_CONFIG_ROOT"] == "/srv/er/configs"
    assert rendered["TRACEPARENT"] == "00-abc-def-01"
    assert container["envFrom"] == [{"secretRef": {"name": "er-erserver-env"}}]


def test_runner_env_sets_class_knobs_when_absent() -> None:
    rendered = {entry["name"]: entry["value"] for entry in runner_env({}, CLASS_S)}
    assert rendered == {"ER_DUCKDB_THREADS": "2", "ER_DUCKDB_MEMORY_LIMIT": "4GB"}


# --- exit-state mapping ------------------------------------------------------


def pod_with(
    exit_code: int | None = None,
    reason: str | None = None,
    pod_reason: str | None = None,
    name: str = "er-run-pod-abcde",
) -> SimpleNamespace:
    terminated = None
    if exit_code is not None or reason is not None:
        terminated = SimpleNamespace(exit_code=exit_code, reason=reason)
    container_statuses = (
        [SimpleNamespace(state=SimpleNamespace(terminated=terminated))] if terminated else []
    )
    return SimpleNamespace(
        metadata=SimpleNamespace(name=name, creation_timestamp="2026-10-10T00:00:00Z"),
        status=SimpleNamespace(
            container_statuses=container_statuses,
            reason=pod_reason,
            message="node pressure" if pod_reason else None,
        ),
    )


def test_engine_exit_codes_pass_through_untouched() -> None:
    for code in (0, 1, 2, 3, 10):
        returncode, _ = pod_exit_state(pod_with(exit_code=code))
        assert returncode == code


def test_every_killed_shape_maps_to_negative_for_the_none_exit_row() -> None:
    # OOMKilled (the §5.3 trap), SIGKILLed (spot/deadline), SIGTERMed, evicted,
    # and pod-lost: all must become exit_code None → requeue with --resume.
    assert pod_exit_state(pod_with(exit_code=137, reason="OOMKilled"))[0] == -9
    assert pod_exit_state(pod_with(exit_code=137, reason="Error"))[0] == -9
    assert pod_exit_state(pod_with(exit_code=143, reason="Error"))[0] == -15
    assert pod_exit_state(pod_with(pod_reason="Evicted"))[0] < 0
    assert pod_exit_state(None)[0] < 0


def test_a_succeeded_job_whose_pod_was_collected_is_still_a_success() -> None:
    returncode, _ = pod_exit_state(None, job_succeeded=True)
    assert returncode == 0


# --- the launcher against a faked API server --------------------------------


class FakeBatch:
    """A scripted batch/v1: read() pops the next status; create/delete record."""

    def __init__(self, reads: list[Any]) -> None:
        self.reads = reads
        self.created: list[dict[str, Any]] = []
        self.deleted: list[str] = []

    def create_namespaced_job(self, namespace: str, body: dict[str, Any]) -> None:
        self.created.append(body)

    def read_namespaced_job(self, name: str, namespace: str) -> Any:
        step = self.reads.pop(0) if len(self.reads) > 1 else self.reads[0]
        if isinstance(step, Exception):
            raise step
        return step

    def delete_namespaced_job(self, name: str, namespace: str, **kwargs: Any) -> None:
        self.deleted.append(name)


class FakeCore:
    def __init__(self, pod: Any | None, log: str = "") -> None:
        self.pod = pod
        self.log = log
        self.log_reads = 0

    def list_namespaced_pod(self, namespace: str, label_selector: str) -> Any:
        return SimpleNamespace(items=[self.pod] if self.pod is not None else [])

    def read_namespaced_pod_log(self, name: str, namespace: str, tail_lines: int) -> str:
        self.log_reads += 1
        return self.log


def active() -> SimpleNamespace:
    return SimpleNamespace(status=SimpleNamespace(succeeded=None, failed=None, conditions=[]))


def succeeded() -> SimpleNamespace:
    return SimpleNamespace(status=SimpleNamespace(succeeded=1, failed=None, conditions=[]))


def failed() -> SimpleNamespace:
    return SimpleNamespace(status=SimpleNamespace(succeeded=None, failed=1, conditions=[]))


RESULT_LINE = (
    '{"run_id":"' + RUN_ID + '","mode":"full","exit_code":0,'
    '"error_class":null,"error_detail":null,"stages":[],"job_id":"' + JOB_ID + '"}'
)
STAGE_LINE = '{"stage":"match","status":"succeeded","exit_code":0,"review_queue_added":3}'


def test_launch_creates_exactly_one_job_and_reads_the_terminal_line() -> None:
    batch = FakeBatch(reads=[active(), succeeded()])
    core = FakeCore(pod_with(exit_code=0), log=f"dbt noise\n{STAGE_LINE}\n{RESULT_LINE}\n")
    launcher = KubernetesLauncher(settings(), batch=batch, core=core)
    staged: list[dict[str, Any]] = []

    result = launcher(payload(), {"ER_CATALOG_DSN": "x"}, on_stage=staged.append)

    assert len(batch.created) == 1
    assert batch.created[0]["metadata"]["name"] == job_name(JOB_ID)
    assert result.returncode == 0
    assert queue.parse_result_line(result.stdout) is not None
    # The trailing S5.2 records are replayed once, at completion — the one log
    # read; progress during the run is the runner's own Postgres write (D3).
    assert staged == [
        {"stage": "match", "status": "succeeded", "exit_code": 0, "review_queue_added": 3}
    ]
    assert core.log_reads == 1
    assert not result.canceled


def test_a_409_on_create_adopts_the_existing_job_instead_of_twinning() -> None:
    batch = FakeBatch(reads=[succeeded()])
    core = FakeCore(pod_with(exit_code=0), log=RESULT_LINE + "\n")
    launcher = KubernetesLauncher(settings(), batch=batch, core=core)

    def create_conflict(namespace: str, body: dict[str, Any]) -> None:
        raise ApiException(status=409, reason="AlreadyExists")

    batch.create_namespaced_job = create_conflict  # type: ignore[method-assign]
    result = launcher(payload(), {})
    assert result.returncode == 0


def test_config_error_surfaces_exit_2_with_the_result_line() -> None:
    line = RESULT_LINE.replace('"exit_code":0', '"exit_code":2').replace(
        '"error_class":null', '"error_class":"config"'
    )
    batch = FakeBatch(reads=[failed()])
    core = FakeCore(pod_with(exit_code=2), log=line + "\n")
    launcher = KubernetesLauncher(settings(), batch=batch, core=core)

    result = launcher(payload(), {})
    assert result.returncode == 2
    parsed = queue.parse_result_line(result.stdout)
    assert parsed is not None and parsed["error_class"] == "config"


def test_an_unreadable_log_still_yields_the_container_exit_code() -> None:
    batch = FakeBatch(reads=[failed()])
    core = FakeCore(pod_with(exit_code=2), log="")

    def no_logs(name: str, namespace: str, tail_lines: int) -> str:
        raise ApiException(status=404, reason="NotFound")

    core.read_namespaced_pod_log = no_logs  # type: ignore[method-assign]
    launcher = KubernetesLauncher(settings(), batch=batch, core=core)
    result = launcher(payload(), {})
    assert result.returncode == 2
    assert queue.parse_result_line(result.stdout) is None


def test_oomkill_returns_negative_so_run_once_requeues_with_resume() -> None:
    batch = FakeBatch(reads=[failed()])
    core = FakeCore(pod_with(exit_code=137, reason="OOMKilled"), log="")
    launcher = KubernetesLauncher(settings(), batch=batch, core=core)

    result = launcher(payload(), {})
    assert result.returncode < 0
    # The policy row this feeds: a dead runner is the infra row.
    from erserver.policy import dispose

    verdict = dispose(None, "infra", attempt=0, max_attempts=3)
    assert verdict.state == QUEUED and verdict.retry_with_resume


def test_cancellation_deletes_the_job_and_reports_canceled() -> None:
    gone = ApiException(status=404, reason="NotFound")
    batch = FakeBatch(reads=[active(), active(), gone])
    core = FakeCore(None)
    launcher = KubernetesLauncher(settings(), batch=batch, core=core)

    result = launcher(payload(), {}, should_cancel=lambda: True)
    assert batch.deleted == [job_name(JOB_ID)]
    assert result.canceled
    assert result.returncode < 0


def test_exists_matches_by_deterministic_name() -> None:
    batch = FakeBatch(reads=[succeeded()])
    launcher = KubernetesLauncher(settings(), batch=batch, core=FakeCore(None))
    assert launcher.exists(JOB_ID)

    missing = FakeBatch(reads=[ApiException(status=404, reason="NotFound")])
    launcher = KubernetesLauncher(settings(), batch=missing, core=FakeCore(None))
    assert not launcher.exists(JOB_ID)


def test_launcher_refuses_to_start_without_a_pinned_image() -> None:
    with pytest.raises(ValueError, match="ERSERVER_RUNNER_IMAGE"):
        KubernetesLauncher(settings(runner_image=None))


# --- startup reconciliation --------------------------------------------------


def job_row(job_id: str, state: str = RUNNING) -> queue.Job:
    return queue.Job(
        job_id=job_id,
        org="acme",
        kind="run_all_full",
        params={},
        state=state,
        priority=0,
        idempotency_key=None,
        run_id=RUN_ID,
        attempt=0,
        max_attempts=3,
        exit_code=None,
        error_class=None,
        error_detail=None,
        outcome=None,
        progress={},
    )


class FakeAdoptable:
    def __init__(self, alive: set[str]) -> None:
        self.alive = alive

    def exists(self, job_id: str) -> bool:
        return job_id in self.alive

    def adopt(self, job_id: str, **kwargs: Any) -> RunnerResult:
        return RunnerResult(returncode=0, stdout="", stderr="")


def test_reconcile_adopts_live_jobs_and_reaps_only_the_lost(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    rows = [job_row("01AAAAAAAAAAAAAAAAAAAAAAAA"), job_row("01BBBBBBBBBBBBBBBBBBBBBBBB")]
    reaped_ids: list[str] = []
    monkeypatch.setattr(dispatcher.queue, "active_jobs", lambda connection: rows)

    def fake_reap(connection: Any, job_ids: list[str]) -> int:
        reaped_ids.extend(job_ids)
        return len(job_ids)

    monkeypatch.setattr(dispatcher.queue, "reap_jobs", fake_reap)

    adopted, reaped = dispatcher.reconcile(
        object(), FakeAdoptable(alive={"01AAAAAAAAAAAAAAAAAAAAAAAA"})
    )
    assert [job.job_id for job in adopted] == ["01AAAAAAAAAAAAAAAAAAAAAAAA"]
    assert reaped == 1
    assert reaped_ids == ["01BBBBBBBBBBBBBBBBBBBBBBBB"]


def test_reconcile_with_nothing_active_is_a_no_op(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(dispatcher.queue, "active_jobs", lambda connection: [])
    monkeypatch.setattr(dispatcher.queue, "reap_jobs", lambda connection, job_ids: len(job_ids))
    adopted, reaped = dispatcher.reconcile(object(), FakeAdoptable(alive=set()))
    assert adopted == [] and reaped == 0


# --- the killed → None seam in the dispatcher's completion -------------------


def test_complete_maps_a_negative_returncode_to_the_none_exit_row(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_finish(connection: Any, job_id: str, disposition: Any, **kwargs: Any) -> None:
        captured["disposition"] = disposition
        captured.update(kwargs)

    monkeypatch.setattr(dispatcher.queue, "finish", fake_finish)
    monkeypatch.setattr(dispatcher.events, "emit", lambda *args, **kwargs: None)

    result = RunnerResult(returncode=-9, stdout="", stderr="runner OOM-killed", canceled=False)
    dispatcher._complete(object(), job_row(JOB_ID), RUN_ID, result, [])

    assert captured["exit_code"] is None
    assert captured["disposition"].state == QUEUED
    assert captured["disposition"].retry_with_resume
    # And once attempts are exhausted, the same row goes terminal.
    exhausted = job_row(JOB_ID)
    exhausted = queue.Job(**{**vars(exhausted), "attempt": 2})
    dispatcher._complete(object(), exhausted, RUN_ID, result, [])
    assert captured["disposition"].state == FAILED
