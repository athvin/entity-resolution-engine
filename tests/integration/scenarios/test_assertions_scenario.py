"""T-ASSERT-1: no active `never` pair shares an entity, in either mode (S8.3, S4.4.2, D5).

M6 restates T-ASSERT-1 as an invariant — "no active `never` pair shares an
`entity_id`, in either mode" — and D5 adds that the outcome must be RECORDED, never
silent: every active `never` pair ends a run in exactly one of {not co-clustered,
cut, escalated}. This module runs `assertions_scenario` through both modes in
separate sub-namespaces and compares the partitions, classifies every `never` pair
with `assert_never_pairs_resolved`, recomputes the A3 cut edge independently and
checks it against the persisted row, exercises the exclusion rule and the
stale-violation recheck, and reaches the escalation branch by raising
`cut_protect_probability` to `auto_merge`.

S8.3 lists T-ASSERT-1 at `tests/integration/test_assertions.py::
test_never_pairs_never_co_cluster`; the board realises it here under the same
function name, and there is no duplicate under `test_assertions.py`.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator, Mapping
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
import yaml
from helpers.invariants import assert_membership_equals_components, assert_never_pairs_resolved
from helpers.model import load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.ids import canonicalize_pair
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.review.never_cut import choose_cut_edge, shortest_path

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
FIXTURE_ROOT: Final = REPO_ROOT / "fixtures" / "static"

SCENARIO: Final = "assertions_scenario"
NON_BATCH: Final = "assertions_non_batch"
BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
CUT_EDGES: Final = f"{SCHEMA_QUALIFIER}.cut_edges"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.review_queue"
ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

#: A3: the `never` whose endpoints stay connected through a third record, so the
#: only way to honour it is a partition-level cut (S4.4.2).
A3_PAIR: Final = ("billing:B703", "crm:C703")
A3_BRIDGE: Final = "webforms:W703"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"


def config() -> Config:
    """The validated S6 document this session runs against (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    """The dbt var payload for one config, from S4.2's one generator."""
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def run_er(*args: str, env: Mapping[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script, in this or a supplied namespace."""
    environ = dict(os.environ)
    if env is not None:
        environ.update(env)
    return subprocess.run(["er", *args], capture_output=True, text=True, env=environ, check=False)


def mutated_config(base: Path, tmp_path: Path, **overrides: Any) -> Path:
    """A temp copy of ``base`` with dotted-path overrides applied (S8.4).

    Local to this module rather than the ER-091 helper it foreshadows: this ticket
    needs exactly one override — `clustering.cut_protect_probability` — to reach the
    escalation branch, and `configs/test.yaml` is never mutated in place.
    """
    document = yaml.safe_load(base.read_text(encoding="utf-8"))
    for dotted, value in overrides.items():
        node = document
        *parents, leaf = dotted.split(".")
        for part in parents:
            node = node[part]
        node[leaf] = value
    out = tmp_path / "mutated.yaml"
    out.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
    return out


class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b)."""

    def __init__(self, connection: duckdb.DuckDBPyConnection, artifacts: Path, cfg: Config):
        self.connection = connection
        self.artifacts = artifacts
        self.cfg = cfg

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
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def deliver(scenario: Scenario, phase: str, root: Path) -> Path:
    for source, path in scenario.inputs_for(phase).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def membership_map(connection: duckdb.DuckDBPyConnection) -> dict[str, str]:
    return {
        str(record): str(entity)
        for record, entity in connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
        ).fetchall()
    }


class Run:
    """Drives one scenario in one namespace: base, assertions, batch, reconcile."""

    def __init__(
        self,
        connection: duckdb.DuckDBPyConnection,
        cfg: Config,
        scenario: Scenario,
        root: Path,
        artifacts: Path,
        *,
        env: Mapping[str, str] | None = None,
        config_path: str | None = None,
    ) -> None:
        self.connection = connection
        self.cfg = cfg
        self.scenario = scenario
        self.root = root
        self.dbt = Dbt(connection, artifacts, cfg)
        self.env = dict(env or {})
        self.config_flag = ["--config", config_path] if config_path else []
        self.run_id = ""

    def _ingest_standardize(self, phase: str) -> None:
        self.run_id = str(ULID())
        delivery = deliver(self.scenario, phase, self.root / phase)
        for source in self.scenario.inputs_for(phase):
            result = run_er(
                "ingest",
                "--source",
                source,
                "--path",
                str(delivery),
                "--run-id",
                self.run_id,
                *self.config_flag,
                env=self.env,
            )
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        self.dbt.standardize()

    def _score_reconcile(self, mode: str) -> None:
        scored = run_er(
            "match", "--mode", mode, "--run-id", self.run_id, *self.config_flag, env=self.env
        )
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        reconciled = run_er("reconcile", "--run-id", self.run_id, *self.config_flag, env=self.env)
        assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr

    def apply_assertions(self, phase: str) -> None:
        for row in self.scenario.assertions.get(phase, ()):
            added = run_er(
                "assert",
                "add",
                "--a",
                str(row["rec_a_key"]),
                "--b",
                str(row["rec_b_key"]),
                "--kind",
                str(row["kind"]),
                "--by",
                str(row["created_by"]),
                "--note",
                str(row["note"]),
                *self.config_flag,
                env=self.env,
            )
            assert added.returncode == int(ExitCode.SUCCESS), added.stdout + added.stderr

    def base(self) -> None:
        self._ingest_standardize(BASE_PHASE)
        self._score_reconcile("full")

    def batch(self, mode: str) -> None:
        self.apply_assertions(BATCH_PHASE)
        self._ingest_standardize(BATCH_PHASE)
        self._score_reconcile(mode)

    def reconcile_again(self) -> str:
        run_id = str(ULID())
        again = run_er("reconcile", "--run-id", run_id, *self.config_flag, env=self.env)
        assert again.returncode in (
            int(ExitCode.SUCCESS),
            int(ExitCode.NOTHING_TO_DO),
        ), again.stdout + again.stderr
        return run_id


def _publish_model(
    connection: duckdb.DuckDBPyConnection, cfg: Config, object_store: ObjectStore
) -> None:
    import json as _json

    model_version, _, settings = load_fixture_model(connection)
    published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
    object_store.put_bytes(published, _json.dumps(settings).encode("utf-8"))
    connection.execute(
        f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
        [published, model_version],
    )


@pytest.fixture(scope="session")
def cfg() -> Config:
    return config()


@pytest.fixture(scope="module")
def dbt_packages() -> None:
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
def scenario_run(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Run]:
    """`assertions_scenario` with its base phase applied, batch pending."""
    scenario = load_scenario(SCENARIO)
    dbt = Dbt(initialised_lake, tmp_path / "artifacts", cfg)
    dbt("seed")
    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        _publish_model(initialised_lake, cfg, object_store)
        run = Run(initialised_lake, cfg, scenario, tmp_path / "drop", tmp_path / "artifacts")
        run.base()
        yield run
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def _recompute_cut_edge(
    connection: duckdb.DuckDBPyConnection, cfg: Config
) -> tuple[str, str, float]:
    """The A3 cut, recomputed from `match_scores` — the independent check for AC3."""
    active = connection.execute(
        f"SELECT model_version, tf_snapshot_id FROM {MODEL_REGISTRY} WHERE status = 'active'"
    ).fetchall()[0]
    rows = connection.execute(
        f"SELECT rec_a_key, rec_b_key, match_probability FROM {MATCH_SCORES} "
        "WHERE is_active AND match_probability >= ? AND model_version = ? AND tf_snapshot_id = ?",
        [cfg.thresholds.auto_merge, str(active[0]), str(active[1])],
    ).fetchall()
    # The clustering graph the stage cut over is assertion-ADJUSTED (S4.4): the A3
    # `never` removes the direct B703–C703 edge, so the endpoints are connected only
    # through the bridge and the shortest path is two hops. Recomputing over the raw
    # `match_scores` would find the 1-hop direct edge and cut the wrong thing — so
    # the recomputation applies the same adjustment by dropping the never pair.
    edges = [
        (str(a), str(b), float(p))
        for a, b, p in rows
        if canonicalize_pair(str(a), str(b)) != A3_PAIR
    ]
    path = shortest_path(A3_PAIR[0], A3_PAIR[1], edges)
    assert path is not None, "A3's endpoints are not connected through the bridge"
    edge = choose_cut_edge(
        path, edges, cut_protect_probability=cfg.clustering.cut_protect_probability
    )
    assert edge is not None, "every edge on A3's path is protected at the default"
    return edge


def _drive_universe(
    universe: Any,
    scenario_name: str,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
    suffix: str,
    mode: str,
    *,
    before_batch: Any = None,
) -> None:
    """Run one scenario end to end in ``universe``, with process env pointed at it.

    The env must be set on the PROCESS, not only passed to `er`: dbt is a subprocess
    that reads `ER_LAKE_*` from the environment, and the sub_namespace factory reverts
    the export after construction. Pointing it here for the arm's duration is what makes
    standardize build into the sub-namespace lake the connection is attached to.
    """
    with pytest.MonkeyPatch.context() as patch:
        for name, value in universe.env.items():
            patch.setenv(name, value)
        _publish_model(universe.connection, cfg, object_store)
        scenario = load_scenario(scenario_name)
        dbt = Dbt(universe.connection, tmp_path / f"artifacts_{suffix}", cfg)
        dbt("seed")
        run = Run(
            universe.connection,
            cfg,
            scenario,
            tmp_path / f"drop_{suffix}",
            tmp_path / f"artifacts_{suffix}",
            env=universe.env,
        )
        run.base()
        if before_batch is not None:
            before_batch(scenario)
        run.batch(mode)


def test_never_pairs_never_co_cluster(
    dbt_packages: None,
    sub_namespace: Any,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> None:
    """T-ASSERT-1 / AC1, AC8: both modes honour every never, and agree."""
    partitions = {}
    for suffix, mode in (("a", "incremental"), ("b", "full")):
        universe = sub_namespace(suffix)
        _drive_universe(universe, SCENARIO, cfg, object_store, tmp_path, suffix, mode)

        membership = membership_map(universe.connection)
        for assertion in universe.connection.execute(
            f"SELECT rec_a_key, rec_b_key FROM {ASSERTIONS} WHERE active AND kind = 'never'"
        ).fetchall():
            left = membership.get(str(assertion[0]))
            right = membership.get(str(assertion[1]))
            assert left is None or right is None or left != right, (
                f"never pair {(assertion[0], assertion[1])} shares an entity in {mode} mode"
            )
        assert_never_pairs_resolved(universe.connection)
        assert_membership_equals_components(universe.connection)
        partitions[suffix] = frozenset(
            frozenset(members) for members in _grouped(membership_map(universe.connection)).values()
        )

    assert partitions["a"] == partitions["b"], (
        "the incremental and full arms disagree on the partition; the never enforcement "
        "is not mode-independent (M6)"
    )


def _grouped(membership: Mapping[str, str]) -> dict[str, set[str]]:
    out: dict[str, set[str]] = {}
    for record, entity in membership.items():
        out.setdefault(entity, set()).add(record)
    return out


def test_every_never_pair_has_a_recorded_outcome(scenario_run: Run) -> None:
    """AC2: the classifier is total and disjoint over the batch state."""
    scenario_run.batch("full")
    outcomes = assert_never_pairs_resolved(scenario_run.connection)
    assert set(outcomes.values()) <= {"not_co_clustered", "cut", "escalated"}
    assert outcomes.get(A3_PAIR) == "cut", (
        f"A3 {A3_PAIR} is {outcomes.get(A3_PAIR)}, not cut; it is the pair whose "
        "endpoints stay connected through a third record and can only be honoured by a cut"
    )


def test_cut_edge_matches_independently_recomputed_choice(scenario_run: Run, cfg: Config) -> None:
    """AC3: the persisted cut is the edge an independent recomputation chooses."""
    scenario_run.batch("full")
    expected = _recompute_cut_edge(scenario_run.connection, cfg)

    persisted = scenario_run.connection.execute(
        f"SELECT rec_a_key, rec_b_key, match_probability FROM {CUT_EDGES} WHERE active"
    ).fetchall()
    assert len(persisted) == 1, f"expected one active cut row, got {persisted}"
    row = (str(persisted[0][0]), str(persisted[0][1]), float(persisted[0][2]))
    assert (row[0], row[1]) == (expected[0], expected[1]), (
        f"the persisted cut {row[:2]} is not the recomputed choice {expected[:2]}"
    )
    assert abs(row[2] - expected[2]) < 1e-9


def test_second_full_run_does_not_undo_the_cut(scenario_run: Run) -> None:
    """AC4: the exclusion rule — a re-run neither re-cuts nor re-merges."""
    scenario_run.batch("full")
    before = membership_map(scenario_run.connection)
    edge_cuts_before = int(
        scalar(
            scenario_run.connection, f"SELECT count(*) FROM {EVENTS} WHERE event_type='edge_cut'"
        )
    )

    rerun = scenario_run.reconcile_again()
    active_cuts = int(
        scalar(scenario_run.connection, f"SELECT count(*) FROM {CUT_EDGES} WHERE active")
    )
    assert active_cuts == 1, f"the re-run left {active_cuts} active cut rows, not 1"
    new_cut_events = int(
        scalar(
            scenario_run.connection,
            f"SELECT count(*) FROM {EVENTS} WHERE event_type='edge_cut' AND run_id = ?",
            rerun,
        )
    )
    assert new_cut_events == 0, "the re-run emitted a fresh edge_cut event"
    assert (
        int(
            scalar(
                scenario_run.connection,
                f"SELECT count(*) FROM {EVENTS} WHERE event_type='edge_cut'",
            )
        )
        == edge_cuts_before
    )
    assert membership_map(scenario_run.connection) == before, "the re-run re-merged the cut pair"
    a3_left = scalar(
        scenario_run.connection,
        f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = ?",
        A3_PAIR[0],
    )
    a3_right = scalar(
        scenario_run.connection,
        f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = ?",
        A3_PAIR[1],
    )
    assert str(a3_left) != str(a3_right), "the cut pair is co-clustered after the re-run"


def test_stale_violation_recheck_against_current_membership(scenario_run: Run) -> None:
    """AC5: retracting the never releases the cut and re-merges; a resolved violation
    is not re-cut."""
    scenario_run.batch("full")
    assertion_id = str(
        scalar(
            scenario_run.connection,
            f"SELECT assertion_id FROM {ASSERTIONS} WHERE active AND kind='never' "
            "AND rec_a_key = ? AND rec_b_key = ?",
            *A3_PAIR,
        )
    )
    removed = run_er("assert", "remove", "--assertion-id", assertion_id, "--by", "tester")
    assert removed.returncode == int(ExitCode.SUCCESS), removed.stdout + removed.stderr

    rerun = scenario_run.reconcile_again()
    released = scenario_run.connection.execute(
        f"SELECT active, released_run_id FROM {CUT_EDGES} WHERE rec_a_key = ? OR rec_b_key = ?",
        [A3_PAIR[0], A3_PAIR[0]],
    ).fetchall()
    assert released, "the cut row vanished instead of being released"
    for active, released_run_id in released:
        assert not active, "the cut is still active after its never was retracted"
        assert str(released_run_id) == rerun, "the release was not stamped with the run"
    a3_left = str(
        scalar(
            scenario_run.connection,
            f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = ?",
            A3_PAIR[0],
        )
    )
    a3_right = str(
        scalar(
            scenario_run.connection,
            f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = ?",
            A3_PAIR[1],
        )
    )
    assert a3_left == a3_right, "the component did not re-merge after the never was retracted"

    # A third run with the never gone and the component already merged re-cuts nothing.
    once_more = scenario_run.reconcile_again()
    new_cuts = int(
        scalar(
            scenario_run.connection,
            f"SELECT count(*) FROM {CUT_EDGES} WHERE cut_run_id = ?",
            once_more,
        )
    )
    assert new_cuts == 0, "a resolved violation was re-cut; the recheck did not run"


def test_non_batch_assertion_matches_full_mode(
    dbt_packages: None,
    sub_namespace: Any,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> None:
    """AC6: assertions_non_batch reaches its pair through the assertion delta alone."""

    def _assert_records_not_in_batch(scenario: Scenario) -> None:
        # The asserted records appear in no batch delivery (S8.2.1) — verified from
        # the committed inputs, so the affected set can only reach them via the delta.
        delivered = {
            f"{source}:{line.split(',')[0]}"
            for source, path in scenario.inputs_for(BATCH_PHASE).items()
            for line in path.read_text(encoding="utf-8").splitlines()[1:]
            if line
        }
        for row in scenario.assertions.get(BATCH_PHASE, ()):
            assert str(row["rec_a_key"]) not in delivered
            assert str(row["rec_b_key"]) not in delivered

    partitions = {}
    for suffix, mode in (("na", "incremental"), ("nb", "full")):
        universe = sub_namespace(suffix)
        _drive_universe(
            universe,
            NON_BATCH,
            cfg,
            object_store,
            tmp_path,
            suffix,
            mode,
            before_batch=_assert_records_not_in_batch,
        )
        assert_never_pairs_resolved(universe.connection)
        assert_membership_equals_components(universe.connection)
        partitions[suffix] = frozenset(
            frozenset(members) for members in _grouped(membership_map(universe.connection)).values()
        )
    assert partitions["na"] == partitions["nb"]


def test_escalation_when_every_path_is_protected(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> None:
    """AC7: at cut_protect_probability = auto_merge, A3 is escalated, not cut."""
    mutated = mutated_config(
        Path(os.environ["ER_CONFIG"]),
        tmp_path,
        **{"clustering.cut_protect_probability": cfg.thresholds.auto_merge},
    )
    strict_cfg = load_config(mutated)
    scenario = load_scenario(SCENARIO)
    dbt = Dbt(initialised_lake, tmp_path / "artifacts", strict_cfg)
    dbt("seed")
    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        _publish_model(initialised_lake, strict_cfg, object_store)
        run = Run(
            initialised_lake,
            strict_cfg,
            scenario,
            tmp_path / "drop",
            tmp_path / "artifacts",
            config_path=str(mutated),
        )
        run.base()
        run.batch("full")

        cut_rows = int(scalar(initialised_lake, f"SELECT count(*) FROM {CUT_EDGES} WHERE active"))
        assert cut_rows == 0, (
            f"{cut_rows} cut rows at cut_protect_probability=auto_merge; every ordinary "
            "edge is protected, so A3 must escalate rather than cut (S4.4.2)"
        )
        escalations = initialised_lake.execute(
            f"SELECT rec_a_key, rec_b_key, subject_type, status FROM {REVIEW_QUEUE} "
            "WHERE reason = 'never_unsatisfiable'"
        ).fetchall()
        assert len(escalations) == 1, f"expected one never_unsatisfiable row, got {escalations}"
        rec_a, rec_b, subject_type, status = escalations[0]
        assert canonicalize_pair(str(rec_a), str(rec_b)) == A3_PAIR
        assert str(subject_type) == "pair"
        assert str(status) == "open"
        assert_membership_equals_components(initialised_lake)
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')
