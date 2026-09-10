"""T-PERM-3: INV-PERM under full re-resolution, literally (S4.5.3, S4.5.4, S8.3).

An incremental history is built over `incremental_batch` (`base/` then `batch/`,
scored through `--mode incremental`), and then a corpus-wide `er match --mode full
&& er reconcile` runs at the SAME `model_version` and `tf_snapshot_id`. INV-PERM is
then asserted clause by clause rather than as "the partitions agree":

* every group of `P_new` set-equal to a group of `P_old` keeps its `entity_id`,
  mints no ULID, and emits **no event** — the third clause is the load-bearing one,
  asserted as a zero event count for those entities under the full run's `run_id`
  and an `entities.updated_run_id` that is not the full run's;
* every entity whose group changed carries a `merged` or `split` event in this
  `run_id`, and no such event names an unchanged entity.

The INV-EQ-adjacent preconditions (same model, TF snapshot, `config_hash`,
`std_version`, active assertion set) are asserted BEFORE the comparison, each with
a message naming the field: a TF change is one of S4.5.6's two loss vectors, and a
divergence it causes must read as a violated precondition, never as a broken
INV-PERM.

The `--reason` arm exercises S4.0's stamping the same way `er correct` will use it
(ER-094): a `never` assertion makes the next reconcile emit real events, and every
one of them — and the `runs` row — must carry `correction_pass`.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.invariants import assert_membership_equals_components
from helpers.model import load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import (
    DBT_PROFILES_DIR,
    DBT_PROJECT_DIR,
    DbtResult,
    render_dbt_vars,
    run_dbt,
)
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

SCENARIO_NAME: Final = "incremental_batch"
BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
RUNS: Final = f"{SCHEMA_QUALIFIER}.runs"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"
ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"

CORRECTION_PASS: Final = "correction_pass"

#: A three-member entity of the batch-phase state (P3's triangle), whose direct
#: edge a `never` suppresses so the reason arm has real events to stamp.
NEVER_PAIR: Final = ("billing:B003", "crm:C003")


def config() -> Config:
    """The validated S6 document this session runs against (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    """The dbt var payload for one config, from S4.2's one generator."""
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def deliver(scenario: Scenario, phase: str, root: Path) -> Path:
    """Materialise one phase as the drop-folder root `er ingest --path` reads."""
    for source, path in scenario.inputs_for(phase).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def membership_pairs(connection: duckdb.DuckDBPyConnection) -> dict[str, str]:
    """`record_key -> entity_id` for every membership row."""
    return {
        str(record): str(entity)
        for record, entity in connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
        ).fetchall()
    }


def groups_of(assignment: dict[str, str]) -> dict[str, frozenset[str]]:
    """`entity_id -> members` for one membership snapshot."""
    grouped: dict[str, set[str]] = {}
    for record, entity in assignment.items():
        grouped.setdefault(entity, set()).add(record)
    return {entity: frozenset(members) for entity, members in grouped.items()}


@dataclass
class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b)."""

    connection: duckdb.DuckDBPyConnection
    artifacts: Path
    cfg: Config

    def __call__(self, command: str, select: str | None = None) -> DbtResult:
        return run_dbt(
            command,
            select=select,
            vars=render_dbt_vars(
                self.cfg, str(ULID()), extra={BLOCKING_DBT_VAR: blocking_payload(self.cfg)}
            ),
            target="lake",
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=DBT_PROJECT_DIR,
            profiles_dir=DBT_PROFILES_DIR,
            artifacts_dir=self.artifacts,
        )

    def standardize(self) -> None:
        """The selection `er standardize` runs: staging, then intermediate (S4.2)."""
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@dataclass
class History:
    """The incremental history, and the state the full pass is compared against."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    scenario: Scenario
    dbt: Dbt
    root: Path
    model_version: str
    tf_snapshot_id: str
    run_ids: list[str] = field(default_factory=list)
    run_id: str = ""

    def phase(self, name: str, *, match_mode: str, reason: str | None = None) -> None:
        """Deliver, ingest, standardize, score and reconcile one phase under one run."""
        self.run_id = str(ULID())
        self.run_ids.append(self.run_id)
        delivery = deliver(self.scenario, name, self.root / name)
        for source in self.scenario.inputs_for(name):
            result = run_er(
                "ingest",
                "--source",
                source,
                "--path",
                str(delivery),
                "--run-id",
                self.run_id,
                "--json",
            )
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        self.dbt.standardize()
        scored = run_er("match", "--mode", match_mode, "--run-id", self.run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        self.reconcile(reason=reason)

    def reconcile(self, *, reason: str | None = None, expect: int = 0) -> None:
        """`er reconcile` under the current run, optionally stamped."""
        args = ["reconcile", "--run-id", self.run_id]
        if reason is not None:
            args += ["--reason", reason]
        reconciled = run_er(*args)
        assert reconciled.returncode == expect, reconciled.stdout + reconciled.stderr

    def full_pass(self, *, reason: str | None = None, expect_reconcile: int = 0) -> str:
        """`er match --mode full && er reconcile` under a fresh run; returns its id."""
        self.run_id = str(ULID())
        self.run_ids.append(self.run_id)
        scored = run_er("match", "--mode", "full", "--run-id", self.run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        self.reconcile(reason=reason, expect=expect_reconcile)
        return self.run_id

    def events_of(self, run_id: str) -> list[tuple[str, str, dict[str, Any]]]:
        """`(entity_id, event_type, details)` for one run."""
        return [
            (str(entity), str(event_type), json.loads(str(details)))
            for entity, event_type, details in self.connection.execute(
                f"SELECT entity_id, event_type, details FROM {EVENTS} WHERE run_id = ? "
                "ORDER BY seq",
                [run_id],
            ).fetchall()
        ]

    def fingerprint(self, run_id: str) -> tuple[str, str | None]:
        """`(config_hash, rebuild_reason)` from the run's `runs` row (S5.1, S5.2)."""
        row = self.connection.execute(
            f"SELECT config_hash, rebuild_reason FROM {RUNS} WHERE run_id = ?", [run_id]
        ).fetchone()
        assert row is not None, f"no runs row for {run_id}"
        return str(row[0]), None if row[1] is None else str(row[1])


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored."""
    if (Path(DBT_PROJECT_DIR) / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = subprocess.run(
        ["dbt", "deps", "--project-dir", DBT_PROJECT_DIR, "--profiles-dir", DBT_PROFILES_DIR],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


@pytest.fixture
def history(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[History]:
    """`incremental_batch` built incrementally: base, then batch, both reconciled."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (BASE_PHASE, BATCH_PHASE)
    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        model_version, tf_snapshot_id, settings = load_fixture_model(initialised_lake)
        published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
        object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
        initialised_lake.execute(
            f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
            [published, model_version],
        )
        built = History(
            connection=initialised_lake,
            cfg=cfg,
            scenario=scenario,
            dbt=dbt,
            root=tmp_path / "drop",
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
        )
        built.phase(BASE_PHASE, match_mode="incremental")
        built.phase(BATCH_PHASE, match_mode="incremental")
        yield built
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def _assert_preconditions(history: History, full_run: str) -> None:
    """AC4: the INV-EQ-adjacent pins, each failure naming its field."""
    active = history.connection.execute(
        f"SELECT model_version, tf_snapshot_id FROM {MODEL_REGISTRY} WHERE status = 'active'"
    ).fetchall()
    assert len(active) == 1, f"{len(active)} active model rows; the pass ran at no pinned model"
    assert str(active[0][0]) == history.model_version, (
        f"model_version moved: history at {history.model_version}, full pass at {active[0][0]}"
    )
    assert str(active[0][1]) == history.tf_snapshot_id, (
        f"tf_snapshot_id moved: history at {history.tf_snapshot_id}, full pass at "
        f"{active[0][1]} — a TF change is an INV-EQ loss vector (S4.5.6), not an "
        "INV-PERM violation"
    )
    incremental_hash, _ = history.fingerprint(history.run_ids[0])
    full_hash, _ = history.fingerprint(full_run)
    assert incremental_hash == full_hash, (
        f"config_hash moved: history {incremental_hash}, full pass {full_hash}"
    )
    assert str(history.cfg.versions.std_version), "std_version unset in the shared document"
    active_assertions = int(
        scalar(history.connection, f"SELECT count(*) FROM {ASSERTIONS} WHERE active")
    )
    assert active_assertions == 0, (
        f"{active_assertions} active assertion(s) differ between the arms; the "
        "INV-PERM comparison requires an identical assertion set"
    )


def test_full_reresolution_satisfies_inv_perm(history: History) -> None:
    """AC1-AC4: the three literal clauses, plus the changed-set converse."""
    p_old = membership_pairs(history.connection)
    ids_before = set(p_old.values())
    updated_before = {
        str(entity): str(run)
        for entity, run in history.connection.execute(
            f"SELECT entity_id, updated_run_id FROM {ENTITIES}"
        ).fetchall()
    }

    full_run = history.full_pass(expect_reconcile=int(ExitCode.NOTHING_TO_DO))
    _assert_preconditions(history, full_run)

    p_new = membership_pairs(history.connection)
    old_groups = groups_of(p_old)
    new_groups = groups_of(p_new)

    unchanged = {
        entity for entity, members in old_groups.items() if new_groups.get(entity) == members
    }
    changed = (set(old_groups) | set(new_groups)) - unchanged

    # Clause 1: a set-equal group keeps its id. Asserted over records so a swap
    # between two same-shaped entities cannot cancel out.
    for record, entity in p_old.items():
        if entity in unchanged:
            assert p_new[record] == entity, (
                f"{record} moved from {entity} to {p_new.get(record)} although its "
                "group is set-equal (INV-PERM clause 1)"
            )

    # Clause 2: no ULID minted for the unchanged world. On this corpus, at the same
    # model and TF snapshot, INV-EQ makes the whole partition set-equal — so the
    # changed set is empty and NO id may be new.
    assert changed == set(), (
        f"the full pass changed {len(changed)} entit(ies): {sorted(changed)}; at an "
        "unchanged model, TF snapshot and assertion set INV-EQ promises equality"
    )
    assert set(p_new.values()) <= ids_before, "the full pass minted an entity id"

    # Clause 3: no event emitted for an unchanged entity, and updated_run_id not
    # touched (AC2).
    emitted = history.events_of(full_run)
    assert emitted == [], f"the full pass emitted {len(emitted)} event(s): {emitted}"
    for entity, run in history.connection.execute(
        f"SELECT entity_id, updated_run_id FROM {ENTITIES}"
    ).fetchall():
        assert str(run) != full_run, f"{entity} carries updated_run_id={full_run}"
        assert str(run) == updated_before[str(entity)]

    assert_membership_equals_components(history.connection)


def test_second_full_pass_is_a_no_op(history: History) -> None:
    """AC8: idempotence — zero events, zero membership rewrites, exit 10."""
    first = history.full_pass(expect_reconcile=int(ExitCode.NOTHING_TO_DO))
    rows_before = history.connection.execute(
        f"SELECT record_key, entity_id, assigned_at, run_id FROM {MEMBERSHIP} ORDER BY record_key"
    ).fetchall()
    events_before = int(scalar(history.connection, f"SELECT count(*) FROM {EVENTS}"))

    second = history.full_pass(expect_reconcile=int(ExitCode.NOTHING_TO_DO))
    assert second != first

    rows_after = history.connection.execute(
        f"SELECT record_key, entity_id, assigned_at, run_id FROM {MEMBERSHIP} ORDER BY record_key"
    ).fetchall()
    assert rows_after == rows_before, "a membership row was rewritten by a no-op pass"
    assert int(scalar(history.connection, f"SELECT count(*) FROM {EVENTS}")) == events_before
    assert history.events_of(second) == []


def test_reason_stamped_on_runs_and_events(history: History) -> None:
    """AC3 (positive arm), AC5, AC6: the stamp on the run row and on every event."""
    # The incremental history itself is ordinary: NULL rebuild_reason and no
    # `reason` key anywhere in its events (AC6).
    for run_id in history.run_ids:
        _, recorded = history.fingerprint(run_id)
        assert recorded is None, f"ordinary run {run_id} carries rebuild_reason={recorded}"
        for entity, event_type, details in history.events_of(run_id):
            assert "reason" not in details, (
                f"ordinary event {event_type} on {entity} carries "
                f"reason={details.get('reason')!r} (S5.1: a reason marks a rebuild)"
            )

    # A `never` on a triangle's direct edge gives the stamped reconcile real work:
    # a cut, a split, and the membership change AC3's positive arm needs.
    p_before = membership_pairs(history.connection)
    assert p_before[NEVER_PAIR[0]] == p_before[NEVER_PAIR[1]], (
        f"{NEVER_PAIR} is not co-clustered; the reason arm would reconcile nothing"
    )
    added = run_er(
        "assert",
        "add",
        "--a",
        NEVER_PAIR[0],
        "--b",
        NEVER_PAIR[1],
        "--kind",
        "never",
        "--by",
        "steward:er-084",
        "--note",
        "T-PERM-3 reason arm: force a stamped reconcile",
    )
    assert added.returncode == int(ExitCode.SUCCESS), added.stdout + added.stderr

    history.run_id = str(ULID())
    history.run_ids.append(history.run_id)
    history.reconcile(reason=CORRECTION_PASS)
    stamped_run = history.run_id

    _, recorded = history.fingerprint(stamped_run)
    assert recorded == CORRECTION_PASS, (
        f"runs.rebuild_reason is {recorded!r}, not {CORRECTION_PASS!r} (AC5)"
    )
    emitted = history.events_of(stamped_run)
    assert emitted, "the stamped reconcile emitted nothing; the never had no effect"
    for entity, event_type, details in emitted:
        assert details.get("reason") == CORRECTION_PASS, (
            f"{event_type} on {entity} carries reason={details.get('reason')!r} (AC5)"
        )

    # AC3's biconditional over the change the never caused: every changed entity
    # carries a split/merged/edge_cut account in this run, and no event names an
    # entity whose group is unchanged.
    p_after = membership_pairs(history.connection)
    old_groups, new_groups = groups_of(p_before), groups_of(p_after)
    unchanged = {
        entity for entity, members in old_groups.items() if new_groups.get(entity) == members
    }
    changed = (set(old_groups) | set(new_groups)) - unchanged
    assert changed, "the never split nothing; the positive arm is vacuous"
    entities_with_events = {entity for entity, _, _ in emitted}
    assert entities_with_events <= changed, (
        f"events name unchanged entit(ies): {entities_with_events - changed}"
    )
    assert changed <= entities_with_events, (
        f"changed entit(ies) with no event in the run: {changed - entities_with_events}"
    )
    split_or_merge = [
        (entity, event_type)
        for entity, event_type, _ in emitted
        if event_type in ("merged", "split")
    ]
    assert split_or_merge, "a changed partition must carry a merged or split event (AC3)"

    assert_membership_equals_components(history.connection)


def test_invalid_reason_exits_two_before_any_connection() -> None:
    """AC7: a malformed --reason is refused at parse time."""
    refused = run_er("reconcile", "--reason", "because-i-said-so")
    assert refused.returncode == 2, refused.stdout + refused.stderr
    combined = refused.stdout + refused.stderr
    assert "rebuild reason" in combined, combined
