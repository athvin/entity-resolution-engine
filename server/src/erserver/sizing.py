"""The §6.5 resource classes: vCPU-tiered, with a near-constant memory floor.

docs/infrastructure.md §6.5 inverts the original memory-tiered resource classes:
the measurements say runtime is CPU-bound and memory is nearly flat in corpus
size, because DuckDB spills to local NVMe rather than growing its working set.
So the ladder is tiered on vCPU/threads, memory barely grows, and ephemeral
storage — the spill — is what actually scales.

Two §6.5 invariants are load-bearing and tested rather than trusted:

* ``ER_DUCKDB_THREADS`` and the CPU request are set in the same operation —
  DuckDB reads neither cgroup limits nor host core count, so either alone is a
  silent misconfiguration.
* ``ER_DUCKDB_MEMORY_LIMIT`` stays strictly below the pod memory limit, or the
  OOM killer replaces the graceful spill.

The class is selected at claim time from the org's last known record count and
written to the job row (``jobs.resource_class``) — per-run sizing rides the job
row, never ``orgs.env``, so a 16-thread nightly run cannot leak its sizing into
the API read path's warm attaches (§18.10).
"""

from __future__ import annotations

from dataclasses import dataclass

__all__ = [
    "CLASS_L",
    "CLASS_M",
    "CLASS_S",
    "DEFAULT_CLASS",
    "RESOURCE_CLASSES",
    "class_named",
    "select_class",
]


@dataclass(frozen=True)
class ResourceClass:
    """One row of the §6.5 table, plus the §6.2 per-class Job knobs."""

    #: Class name, as stored on ``jobs.resource_class``.
    name: str
    #: Upper bound (inclusive) of the record counts this class serves;
    #: ``None`` for the top of the ladder.
    max_records: int | None
    #: CPU request, in Kubernetes quantity form. Matches ``duckdb_threads`` —
    #: the two must move together (§6.5).
    cpu: str
    #: Pod memory request and limit. Strictly above ``duckdb_memory_limit``:
    #: the gap is Python heap, Arrow buffers, and the dbt subprocess.
    memory: str
    #: ``ER_DUCKDB_THREADS`` for the run.
    duckdb_threads: int
    #: ``ER_DUCKDB_MEMORY_LIMIT`` for the run (DuckDB's own syntax, e.g. "4GB").
    duckdb_memory_limit: str
    #: ``requests.ephemeral-storage`` == ``limits.ephemeral-storage`` and each
    #: spill emptyDir's ``sizeLimit`` — sized from measured spill (§5.3), and
    #: what lets the scheduler avoid stacking two spill-heavy pods on one node.
    ephemeral_storage: str
    #: The Karpenter NodePool this class schedules onto (§5).
    nodepool: str
    #: ``activeDeadlineSeconds`` for the Job (§6.2: S 2h / M 6h / L 24h).
    active_deadline_seconds: int


#: The §6.5 table, verbatim. L takes the low end of the 12–16 vCPU band until
#: §18.2's thread-knee measurement says more threads buy anything — the same
#: run already showed reconcile getting *slower* at higher thread counts.
CLASS_S = ResourceClass(
    name="S",
    max_records=100_000,
    cpu="2",
    memory="6Gi",
    duckdb_threads=2,
    duckdb_memory_limit="4GB",
    ephemeral_storage="40Gi",
    nodepool="runner-sm",
    active_deadline_seconds=2 * 60 * 60,
)
CLASS_M = ResourceClass(
    name="M",
    max_records=1_000_000,
    cpu="6",
    memory="10Gi",
    duckdb_threads=6,
    duckdb_memory_limit="6GB",
    ephemeral_storage="150Gi",
    nodepool="runner-sm",
    active_deadline_seconds=6 * 60 * 60,
)
CLASS_L = ResourceClass(
    name="L",
    max_records=None,
    cpu="12",
    memory="14Gi",
    duckdb_threads=12,
    duckdb_memory_limit="8GB",
    ephemeral_storage="400Gi",
    nodepool="runner-l",
    active_deadline_seconds=24 * 60 * 60,
)

RESOURCE_CLASSES: tuple[ResourceClass, ...] = (CLASS_S, CLASS_M, CLASS_L)

#: For an org with no completed pipeline run there is no record count to key
#: on. M is the deliberate compromise: S would OOM a first 10M-record load
#: (the measured 10M peak is 9.37 GiB against S's 6 GiB), L buys a 16-vCPU
#: node for what is usually a trial corpus. The first completed run leaves its
#: row counts in the job ledger, and every later claim re-classes from those.
DEFAULT_CLASS = CLASS_M

#: Lifecycle work (CREATE DATABASE, seed config, ``er init``) touches no
#: corpus; it always takes the smallest class regardless of history.
_LIFECYCLE_KINDS = frozenset({"provision"})


def class_named(name: str) -> ResourceClass | None:
    """The class called ``name``, or ``None`` — never a KeyError at dispatch."""
    for cls in RESOURCE_CLASSES:
        if cls.name == name:
            return cls
    return None


def select_class(
    records: int | None,
    *,
    kind: str | None = None,
    override: object = None,
) -> ResourceClass:
    """The §6.5 class for a run over ``records`` records.

    ``override`` is a per-run escape hatch: a job whose params carry
    ``resource_class`` (an operator pinning a known-heavy tenant) takes that
    class verbatim. An unrecognized override is ignored rather than fatal — a
    typo in an operator knob must not fail the tenant's run.
    """
    if isinstance(override, str):
        chosen = class_named(override)
        if chosen is not None:
            return chosen
    if kind in _LIFECYCLE_KINDS:
        return CLASS_S
    if records is None:
        return DEFAULT_CLASS
    for cls in RESOURCE_CLASSES:
        if cls.max_records is not None and records <= cls.max_records:
            return cls
    return CLASS_L
