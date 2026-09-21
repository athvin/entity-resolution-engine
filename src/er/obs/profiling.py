"""Opt-in, append-only transformation traces. Never writes pipeline data or stdout."""

from __future__ import annotations

import contextvars
import dataclasses
import functools
import inspect
import json
import os
import subprocess
import sys
import threading
import time
import uuid
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, ParamSpec, TypeVar

P = ParamSpec("P")
T = TypeVar("T")
_current: contextvars.ContextVar[dict[str, Any] | None] = contextvars.ContextVar(
    "er_profile_span", default=None
)
_write_lock = threading.Lock()


def enabled() -> bool:
    return bool(os.environ.get("ER_PROFILE_DIR"))


def context() -> dict[str, Any]:
    return dict(
        _current.get()
        or {
            "run_id": os.environ.get("ER_PROFILE_RUN_ID"),
            "span_id": os.environ.get("ER_PROFILE_PARENT_SPAN"),
            "stage": os.environ.get("ER_PROFILE_STAGE"),
        }
    )


def emit(event: str, **values: Any) -> None:
    if not enabled():
        return
    directory = Path(os.environ["ER_PROFILE_DIR"])
    directory.mkdir(parents=True, exist_ok=True)
    record = {
        "schema_version": 1,
        "event": event,
        "session_id": os.environ.get("ER_PROFILE_SESSION_ID", directory.name),
        "invocation_id": os.environ.get("ER_PROFILE_INVOCATION_ID"),
        "pid": os.getpid(),
        "timestamp": datetime.now(UTC).isoformat(),
        "monotonic_ns": time.monotonic_ns(),
        **context(),
        **values,
    }
    line = json.dumps(record, default=str, separators=(",", ":")) + "\n"
    with _write_lock, (directory / f"events-{os.getpid()}.jsonl").open("a") as handle:
        handle.write(line)


def cgroup_readings() -> dict[str, int | None]:
    root = Path("/sys/fs/cgroup")
    readings: dict[str, int | None] = {}
    for name in ("memory.current", "memory.peak", "memory.max"):
        try:
            raw = (root / name).read_text().strip()
            readings[name] = int(raw) if raw != "max" else None
        except (OSError, ValueError):
            readings[name] = None
    try:
        for line in (root / "cpu.stat").read_text().splitlines():
            key, value = line.split()
            readings[key] = int(value)
    except (OSError, ValueError):
        pass
    return readings


@contextmanager
def span(name: str, *, unit: str | None = None, **attributes: Any) -> Iterator[dict[str, Any]]:
    metrics: dict[str, Any] = {}
    if not enabled():
        yield metrics
        return
    parent = context()
    identity = {
        **parent,
        "span_id": uuid.uuid4().hex,
        "parent_span_id": parent.get("span_id"),
        "name": name,
        "unit": unit,
        **attributes,
    }
    token = _current.set(identity)
    start = time.monotonic_ns()
    cpu = cgroup_readings()
    emit("span_start", **identity)
    status = "succeeded"
    try:
        yield metrics
    except BaseException as error:
        status = "failed"
        metrics.update(error_type=type(error).__name__, error_detail=str(error))
        raise
    finally:
        if metrics.get("exit_code", 0) not in (0, 10):
            status = "failed"
        elapsed = (time.monotonic_ns() - start) / 1_000_000
        end_cpu = cgroup_readings()
        for key in ("usage_usec", "user_usec", "system_usec", "throttled_usec"):
            before, after = cpu.get(key), end_cpu.get(key)
            metrics[key] = None if before is None or after is None else max(0, after - before)
        emit("span_end", **identity, duration_ms=elapsed, status=status, metrics=metrics)
        _current.reset(token)


def numeric_result(result: Any) -> dict[str, Any]:
    """Counts only: never serialize record payloads, names, keys, or model settings."""
    if dataclasses.is_dataclass(result) and not isinstance(result, type):
        counts = {
            field.name: value
            for field in dataclasses.fields(result)
            if isinstance(value := getattr(result, field.name), (int, float, bool))
        }
        for field in dataclasses.fields(result):
            value = getattr(result, field.name)
            if isinstance(value, (list, tuple, set, frozenset, dict)):
                counts[f"{field.name}_count"] = len(value)
        return counts
    if isinstance(result, (int, float)):
        return {"result_count": result}
    if isinstance(result, (list, tuple, set, frozenset, dict)):
        return {"result_count": len(result)}
    return {}


def profiled(name: str, unit: str = "records") -> Callable[[Callable[P, T]], Callable[P, T]]:
    def decorate(function: Callable[P, T]) -> Callable[P, T]:
        signature = inspect.signature(function)

        @functools.wraps(function)
        def wrapped(*args: P.args, **kwargs: P.kwargs) -> T:
            if not enabled():
                return function(*args, **kwargs)
            bound = signature.bind(*args, **kwargs)
            attributes = {
                key: value
                for key in ("source", "mode", "model_version", "tf_snapshot_id")
                if isinstance(value := bound.arguments.get(key), str)
            }
            with span(name, unit=unit, **attributes) as metrics:
                for key in ("nodes", "edges", "pairs", "record_keys", "assertions"):
                    value = bound.arguments.get(key)
                    if isinstance(value, (list, tuple, set, frozenset, dict)):
                        metrics[f"{key}_in"] = len(value)
                result = function(*args, **kwargs)
                metrics.update(numeric_result(result))
                return result

        return wrapped

    return decorate


class ResourceSampler:
    """Container totals include every CLI/dbt child; RSS is a separate process series."""

    def __init__(self, destination: Path, interval: float = 0.25) -> None:
        self.destination = destination
        self.interval = interval
        self.stop_event = threading.Event()
        self.thread: threading.Thread | None = None
        self.error: str | None = None

    def start(self) -> None:
        self.destination.parent.mkdir(parents=True, exist_ok=True)
        self.thread = threading.Thread(target=self._run, daemon=True, name="er-resources")
        self.thread.start()

    def _sample(self) -> dict[str, Any]:
        rss: dict[str, int] = {}
        for status in Path("/proc").glob("[0-9]*/status"):
            try:
                for line in status.read_text().splitlines():
                    if line.startswith("VmRSS:"):
                        rss[status.parent.name] = int(line.split()[1]) * 1024
            except (OSError, ValueError, ProcessLookupError):
                continue
        return {
            "timestamp": datetime.now(UTC).isoformat(),
            "monotonic_ns": time.monotonic_ns(),
            **cgroup_readings(),
            "process_rss_bytes": rss,
        }

    def _run(self) -> None:
        try:
            with self.destination.open("a") as handle:
                while True:
                    handle.write(json.dumps(self._sample()) + "\n")
                    handle.flush()
                    if self.stop_event.wait(self.interval):
                        handle.write(json.dumps(self._sample()) + "\n")
                        break
        except Exception as error:
            self.error = f"{type(error).__name__}: {error}"

    def stop(self) -> None:
        self.stop_event.set()
        if self.thread is not None:
            self.thread.join(timeout=5)
            if self.thread.is_alive():
                self.error = "resource sampler did not stop"
        if self.error:
            emit("sampler_error", error_detail=self.error)


def stream_command(
    argv: Sequence[str],
    *,
    directory: Path,
    env: Mapping[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Drain both pipes concurrently; retain bounded tails and full durable logs."""
    directory.mkdir(parents=True, exist_ok=True)
    child_env = dict(os.environ if env is None else env)
    current = context()
    for key, field in (
        ("ER_PROFILE_PARENT_SPAN", "span_id"),
        ("ER_PROFILE_RUN_ID", "run_id"),
        ("ER_PROFILE_STAGE", "stage"),
    ):
        if current.get(field) is not None:
            child_env[key] = str(current[field])
    tails: dict[str, str] = {"stdout": "", "stderr": ""}
    errors: list[BaseException] = []
    process = subprocess.Popen(
        list(argv),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=child_env,
        bufsize=1,
    )

    def drain(stream_name: str) -> None:
        stream = getattr(process, stream_name)
        try:
            with (directory / f"{stream_name}.log").open("w") as log:
                while chunk := stream.readline(65536):
                    log.write(chunk)
                    log.flush()
                    tails[stream_name] = (tails[stream_name] + chunk)[-65536:]
                    # Preserve the parent's command-output protocol.
                    sys.stderr.write(chunk)
                    sys.stderr.flush()
        except BaseException as error:
            errors.append(error)
            process.terminate()
        finally:
            stream.close()

    threads = [threading.Thread(target=drain, args=(name,), daemon=True) for name in tails]
    for thread in threads:
        thread.start()
    try:
        while True:
            try:
                code = process.wait(timeout=15)
                break
            except subprocess.TimeoutExpired:
                print(
                    f"[profile] still running: {argv[0]} (pid {process.pid})",
                    file=sys.stderr,
                    flush=True,
                )
    except BaseException:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        raise
    finally:
        for thread in threads:
            thread.join(timeout=5)
    if errors or any(thread.is_alive() for thread in threads):
        raise RuntimeError(f"subprocess log capture failed: {errors or 'pipe did not close'}")
    return subprocess.CompletedProcess(list(argv), code, tails["stdout"], tails["stderr"])
