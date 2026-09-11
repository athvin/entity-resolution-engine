"""The one parser of `.github/workflows/benchmark.yaml` (S9.2, M24/M23/M25).

ER-102's `--validate-baselines` and this ticket's tests both read the benchmark workflow —
its dispatch options, per-scale runner and timeout, the envelope the preflight exports, and
its `uses:` pins — so they read it through `parse_benchmark_workflow` rather than two
independent greps. The workflow's `runs-on`/`timeout-minutes` are GitHub Actions ternary
expressions over `inputs.scale`; :class:`WorkflowEnvelope` evaluates the specific
`(scale == 'X') && A || B` shape so a caller asks `runner_for('100k')` instead of parsing
the expression itself.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml

__all__ = [
    "UsesPin",
    "WorkflowEnvelope",
    "dispatch_options",
    "parse_benchmark_workflow",
    "preflight_script",
]

#: A GitHub ternary over the scale: `(inputs.scale == 'X') && <true> || <false>`.
_TERNARY = re.compile(
    r"inputs\.scale\s*==\s*'(?P<scale>[^']+)'\s*\)\s*&&\s*"
    r"(?P<true>'[^']+'|\d+)\s*\|\|\s*(?P<false>'[^']+'|\d+)"
)
# The ref is captured as any non-space token (SHA or tag) so a tag pin is parsed AND then
# rejected by `is_sha_pinned`, rather than silently not matching.
_USES = re.compile(r"uses:\s*(?P<action>[A-Za-z0-9._/-]+)@(?P<sha>[^\s#]+)\s*#\s*(?P<comment>\S+)")
_ENV_ASSIGN = re.compile(r'echo\s+"(?P<name>[A-Z_]+)=')


@dataclass(frozen=True)
class UsesPin:
    """One `uses:` reference: the action, its pinned SHA, and the tag comment."""

    action: str
    sha: str
    comment: str

    @property
    def is_sha_pinned(self) -> bool:
        """A 40-hex SHA with a `v…` tag comment (the S9 rule)."""
        return bool(re.fullmatch(r"[0-9a-f]{40}", self.sha)) and self.comment.startswith("v")


def _ternary(expression: str, scale: str) -> str | None:
    match = _TERNARY.search(expression)
    if match is None:
        return None
    chosen = match.group("true") if scale == match.group("scale") else match.group("false")
    return chosen.strip("'")


@dataclass(frozen=True)
class WorkflowEnvelope:
    """The parsed benchmark workflow, read once for every consumer (S9.2)."""

    dispatch_options: frozenset[str]
    scheduled_crons: tuple[str, ...]
    permissions_contents: str | None
    concurrency_cancel_in_progress: bool
    runs_on_expr: str
    timeout_expr: str
    uses_pins: tuple[UsesPin, ...]
    preflight_script: str
    preflight_env_fields: frozenset[str]
    steps: tuple[dict[str, Any], ...]
    compare_step_index: int
    upload_step_index: int
    compare_command: str
    upload_if: str | None
    upload_if_no_files_found: str | None
    teardown_if: str | None

    def runner_for(self, scale: str) -> str | None:
        return _ternary(self.runs_on_expr, scale)

    def timeout_for(self, scale: str) -> int | None:
        value = _ternary(self.timeout_expr, scale)
        return None if value is None else int(value)


def _on_block(document: dict[Any, Any]) -> dict[str, Any]:
    """The `on:` mapping — YAML 1.1 parses the bare key `on` as the boolean ``True``."""
    block = document.get("on", document.get(True))
    if not isinstance(block, dict):
        raise ValueError("benchmark workflow has no `on:` mapping")
    return block


def parse_benchmark_workflow(
    path: str | Path = ".github/workflows/benchmark.yaml",
) -> WorkflowEnvelope:
    """Parse the benchmark workflow into a :class:`WorkflowEnvelope`.

    Raises:
        ValueError: the file is missing the `on:`, `jobs.bench`, or the steps this
            workflow is required to carry.
    """
    text = Path(path).read_text(encoding="utf-8")
    document = yaml.safe_load(text)
    on_block = _on_block(document)

    dispatch = on_block.get("workflow_dispatch") or {}
    options = (((dispatch.get("inputs") or {}).get("scale") or {}).get("options")) or []
    crons = tuple(str(entry["cron"]) for entry in (on_block.get("schedule") or []))

    permissions = document.get("permissions") or {}
    concurrency = document.get("concurrency") or {}
    job = (document.get("jobs") or {}).get("bench")
    if not isinstance(job, dict):
        raise ValueError("benchmark workflow has no `jobs.bench`")
    steps = tuple(job.get("steps") or ())

    def _step_index(predicate: Any) -> int:
        for index, step in enumerate(steps):
            if predicate(step):
                return index
        return -1

    compare_index = _step_index(lambda s: "--compare" in str(s.get("run", "")))
    upload_index = _step_index(lambda s: "upload-artifact@" in str(s.get("uses", "")))
    preflight_index = _step_index(lambda s: "Preflight" in str(s.get("name", "")))
    teardown_index = _step_index(
        lambda s: "down -v" in str(s.get("run", "")) and "Teardown" in str(s.get("name", ""))
    )

    preflight = steps[preflight_index].get("run", "") if preflight_index >= 0 else ""
    upload = steps[upload_index] if upload_index >= 0 else {}
    with_block = upload.get("with") or {}

    return WorkflowEnvelope(
        dispatch_options=frozenset(str(option) for option in options),
        scheduled_crons=crons,
        permissions_contents=permissions.get("contents"),
        concurrency_cancel_in_progress=bool(concurrency.get("cancel-in-progress", True)),
        runs_on_expr=str(job.get("runs-on", "")),
        timeout_expr=str(job.get("timeout-minutes", "")),
        uses_pins=tuple(
            UsesPin(m.group("action"), m.group("sha"), m.group("comment"))
            for m in _USES.finditer(text)
        ),
        preflight_script=str(preflight),
        preflight_env_fields=frozenset(m.group("name") for m in _ENV_ASSIGN.finditer(preflight)),
        steps=steps,
        compare_step_index=compare_index,
        upload_step_index=upload_index,
        compare_command=str(steps[compare_index].get("run", "")) if compare_index >= 0 else "",
        upload_if=upload.get("if"),
        upload_if_no_files_found=with_block.get("if-no-files-found"),
        teardown_if=steps[teardown_index].get("if") if teardown_index >= 0 else None,
    )


def dispatch_options(path: str | Path = ".github/workflows/benchmark.yaml") -> frozenset[str]:
    """The scales the workflow offers for dispatch — the committed baseline set (S9.2)."""
    return parse_benchmark_workflow(path).dispatch_options


def preflight_script(path: str | Path = ".github/workflows/benchmark.yaml") -> str:
    """The preflight step's shell, for executing under a stub `docker` (S9.2)."""
    return parse_benchmark_workflow(path).preflight_script
