"""S4.3.2's call sequence, asserted three ways: against the spec, a spy, and itself.

The sequence is the ticket. M8 is not a bug in an argument value — it is that two of
the three estimators had required arguments the old config could not supply, and that
`estimate_u_using_random_sampling` ran unseeded, so the same corpus and the same
config produced a different model each time. None of that is visible in a model that
merely loads: an unseeded u estimate produces a *plausible* model, and a single EM
session produces one whose blocked column's m is simply the prior.

So the three assertions are layered deliberately:

* :func:`test_declaration_matches_spec_s4_3_2` parses the fenced Python block under
  the S4.3.2 anchor of DesignDoc.md and compares it to
  :data:`~er.matching.train.TRAIN_CALL_SEQUENCE`. Without it the declaration would be
  its own authority, and "the sequence is fixed" would mean "the sequence is whatever
  `train.py` says".
* The spy tests assert what `train_model` *did*: a recorder that answers to any
  attribute, so an undeclared call is recorded rather than raising, and the claim
  "exactly these calls, in this order, with these keywords" is made against evidence.
* :func:`test_repeat_training_is_bit_identical` runs the real thing twice. A spy
  cannot show that Splink itself is reproducible under the seed, and reproducibility
  is what the seed exists for.

Nothing here needs Docker. The lake is an in-memory database ATTACHed under the alias,
`int_std_records` is a local table with the S4.3.1 columns `configs/test.yaml`
references, and the training corpus is a local copy of it — which is not a shortcut
but S4.0b's own shape for a Splink input ("materialized into local temp tables first").
"""

from __future__ import annotations

import ast
import os
import re
from collections.abc import Iterator
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
import yaml

from er.config.loader import load_config
from er.config.schema import Config
from er.lake.model import REGISTRY, create_table_sql
from er.matching.api import SPLINK_SCRATCH_SCHEMA
from er.matching.tf import TF_LOOKUP_RELATION, tf_columns
from er.matching.train import (
    EM_RULE,
    NON_FITTED_SETTINGS_KEYS,
    SETTINGS_READ_PATH,
    TRAIN_CALL_SEQUENCE,
    TrainCall,
    TrainCallSpec,
    build_training_linker,
    render_call_sequence,
    train_model,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"
TRAIN_MODULE = REPO_ROOT / "src" / "er" / "matching" / "train.py"

#: The bare local relation name the linker trains over, per S4.0b.
CORPUS: Final = "er_054_corpus"

#: `model_version` is ER-055's to allocate; this suite only needs a key for the frozen
#: TF rows, in the zero-padded shape S4.3.2 step 2 gives it.
MODEL_VERSION: Final = "v0001"

#: The construction AC6 forbids `train.py` from making. Held as a name and turned into
#: a call pattern below, so this module never contains a call site of its own subject.
DUCKDB_API: Final = "DuckDBAPI"

#: What the spy hands back when the fitted model is read off it. Shaped like a Splink
#: settings document only in the one key `train_model` reads for its metrics.
SPY_SETTINGS: Final[dict[str, Any]] = {
    "link_type": "dedupe_only",
    "probability_two_random_records_match": 0.0004,
    "comparisons": [],
}

#: The exact sequence `configs/test.yaml` renders to: two singletons and one session
#: per `em_blocking_rules` entry. Written out rather than derived from the config,
#: because a test that recomputes its expectation from the file under test asserts
#: nothing about that file.
EXPECTED_CALLS: Final[tuple[TrainCall, ...]] = (
    TrainCall(
        "training.estimate_probability_two_random_records_match",
        {
            "deterministic_matching_rules": [
                "l.email = r.email",
                "l.phone_e164 = r.phone_e164",
                "l.family_name = r.family_name and l.birth_date = r.birth_date",
            ],
            "recall": 0.85,
        },
    ),
    TrainCall(
        "training.estimate_u_using_random_sampling",
        {"max_pairs": 1000000, "seed": 20260101},
    ),
    TrainCall(
        "training.estimate_parameters_using_expectation_maximisation",
        {"blocking_rule": "l.email = r.email", "fix_u_probabilities": True},
    ),
    TrainCall(
        "training.estimate_parameters_using_expectation_maximisation",
        {
            "blocking_rule": "l.family_name = r.family_name and l.addr_postal = r.addr_postal",
            "fix_u_probabilities": True,
        },
    ),
)

#: Only the `int_std_records` columns `configs/test.yaml` references: its four
#: `blocking:` expressions and its six `comparisons:` keys, plus `record_key` as
#: `unique_id_column_name`. `birth_date` is a real `DATE` because the
#: `dob_same_year_month` level renders `date_trunc('month', birth_date_l)`.
STD_RECORDS_DDL: Final = """
CREATE TABLE lake.main.int_std_records (
    record_key    VARCHAR,
    given_name    VARCHAR,
    family_name   VARCHAR,
    name_variants VARCHAR[],
    email         VARCHAR,
    phone_e164    VARCHAR,
    birth_date    DATE,
    addr_postal   VARCHAR
)
"""

#: Twelve personas, two records each. Two records per persona is what makes EM have
#: anything to estimate; the corruptions below are what stop every comparison from
#: landing on its exact level, which would leave every other level with a zero count
#: and an m that EM never moved.
PERSONA_COUNT: Final = 12


def corpus_rows() -> list[tuple[Any, ...]]:
    """The frozen corpus, in `record_key` order and identical on every call."""
    rows: list[tuple[Any, ...]] = []
    for persona in range(PERSONA_COUNT):
        given = f"person{persona}"
        family = f"sur{persona}"
        email = f"p{persona}@example.com"
        phone = f"+1555000{persona:04d}"
        postal = f"{10000 + persona}"
        birth = date(1990, 1, persona % 27 + 1)
        rows.append((f"crm:{persona:02d}", given, family, [given], email, phone, birth, postal))
        # The duplicate, corrupted on a different attribute every few personas: a
        # near-miss given name reaches the jaro_winkler level, a different address
        # reaches the else level, a missing phone reaches the null level, and a
        # birth date in the same month reaches `dob_same_year_month`.
        rows.append(
            (
                f"billing:{persona:02d}",
                given if persona % 3 else f"{given}x",
                family,
                [given],
                email if persona % 4 else f"{given}@other.example.com",
                phone if persona % 5 else None,
                birth if persona % 2 else date(1990, 1, (persona + 13) % 27 + 1),
                postal,
            )
        )
    return rows


def document() -> dict[str, Any]:
    """A fresh, mutable copy of the shipped test document."""
    loaded = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict), "configs/test.yaml is not a mapping"
    return loaded


def scalar(connection: duckdb.DuckDBPyConnection, statement: str) -> Any:
    row = connection.execute(statement).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


@dataclass
class SpyNamespace:
    """One namespace of the spy linker, recording every attribute it is asked for.

    The `__getattr__` fallback is the point: a Splink 3-style call, a fourth estimator
    or a stray `compute_tf_table` is *recorded* rather than raising, so "exactly these
    calls" is a claim about what happened rather than about what did not crash.
    """

    name: str
    calls: list[TrainCall]
    returns: dict[str, Any] = field(default_factory=dict)

    def __getattr__(self, attribute: str) -> Any:
        def recorder(**kwargs: Any) -> Any:
            self.calls.append(TrainCall(f"{self.name}.{attribute}", dict(kwargs)))
            return self.returns.get(attribute)

        return recorder


@dataclass
class SpyLinker:
    """A `Linker` that records the call sequence instead of fitting anything."""

    calls: list[TrainCall] = field(default_factory=list)
    settings: dict[str, Any] = field(default_factory=lambda: dict(SPY_SETTINGS))

    @property
    def training(self) -> SpyNamespace:
        return SpyNamespace("training", self.calls)

    @property
    def misc(self) -> SpyNamespace:
        return SpyNamespace("misc", self.calls, {"save_model_to_json": self.settings})


def spec_call_sequence() -> tuple[TrainCallSpec, ...]:
    """The fenced Python block under S4.3.2, parsed into the same shape as the declaration.

    Parsed rather than pattern-matched: the block is executable Python, and its
    structure — three statements, the third a `for` over `cfg.training.em_blocking_rules`
    — is exactly what the declaration claims. `ast` reads that structure; a regex would
    read the text it happens to be written in.
    """
    text = DESIGN_DOC.read_text(encoding="utf-8")
    anchor = text.find('<a id="s4-3-2"></a>')
    assert anchor != -1, "DesignDoc.md has no S4.3.2 anchor"
    fence = text.find("```python", anchor)
    assert fence != -1, "S4.3.2 has no fenced python block"
    body_start = fence + len("```python")
    body_end = text.find("```", body_start)
    assert body_end != -1, "the S4.3.2 python block is unterminated"

    module = ast.parse(text[body_start:body_end])
    specs: list[TrainCallSpec] = []
    for statement in module.body:
        if isinstance(statement, ast.Expr) and isinstance(statement.value, ast.Call):
            specs.append(_spec_from_call(statement.value, loop_variable=None, repeat_over=None))
            continue
        assert isinstance(statement, ast.For), f"unexpected {type(statement).__name__} in S4.3.2"
        assert isinstance(statement.target, ast.Name)
        assert len(statement.body) == 1
        inner = statement.body[0]
        assert isinstance(inner, ast.Expr) and isinstance(inner.value, ast.Call)
        specs.append(
            _spec_from_call(
                inner.value,
                loop_variable=statement.target.id,
                repeat_over=_config_path(statement.iter),
            )
        )
    return tuple(specs)


def _dotted(node: ast.expr) -> str:
    """An attribute chain such as `linker.training.estimate_u_using_random_sampling`."""
    if isinstance(node, ast.Name):
        return node.id
    assert isinstance(node, ast.Attribute), f"not an attribute chain: {ast.dump(node)}"
    return f"{_dotted(node.value)}.{node.attr}"


def _config_path(node: ast.expr) -> str:
    """A `cfg.…` reference as the dotted path the declaration stores."""
    path = _dotted(node)
    assert path.startswith("cfg."), f"S4.3.2 argument {path!r} does not come from the config"
    return path.removeprefix("cfg.")


def _spec_from_call(
    call: ast.Call, *, loop_variable: str | None, repeat_over: str | None
) -> TrainCallSpec:
    assert not call.args, "S4.3.2 passes every argument by keyword"
    kwargs: dict[str, str] = {}
    for keyword in call.keywords:
        assert keyword.arg is not None, "S4.3.2 uses no ** expansion"
        value = keyword.value
        if loop_variable is not None and isinstance(value, ast.Name) and value.id == loop_variable:
            kwargs[keyword.arg] = EM_RULE
        else:
            kwargs[keyword.arg] = _config_path(value)
    method_path = _dotted(call.func)
    assert method_path.startswith("linker."), f"{method_path!r} is not a call on the linker"
    return TrainCallSpec(method_path.removeprefix("linker."), kwargs, repeat_over)


def call_sites(source: Path, token: str) -> tuple[int, ...]:
    """The line numbers in `source` holding a call to `token`."""
    pattern = re.compile(rf"\b{re.escape(token)}\s*\(")
    lines = source.read_text(encoding="utf-8").splitlines()
    return tuple(n for n, line in enumerate(lines, 1) if pattern.search(line))


@pytest.fixture
def cfg() -> Config:
    """The validated S6 document `configs/test.yaml` holds."""
    return load_config(TEST_CONFIG)


@pytest.fixture
def corpus(monkeypatch: pytest.MonkeyPatch) -> Iterator[duckdb.DuckDBPyConnection]:
    """An S4.0b-shaped connection: `:memory:` primary, lake by alias, corpus local.

    The lake holds `int_std_records` and `tf_lookup` because that is where
    `materialize_tf_lookup` reads and writes; the training corpus is a *local* copy,
    because Splink resolves an input table through
    `information_schema.columns WHERE table_name = '<name>'` and a lake-qualified name
    matches no row there.
    """
    monkeypatch.setenv("ER_DUCKDB_THREADS", "2")
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute(create_table_sql(REGISTRY[TF_LOOKUP_RELATION]))
    connection.execute(STD_RECORDS_DDL)
    connection.executemany(
        "INSERT INTO lake.main.int_std_records VALUES (?, ?, ?, ?, ?, ?, ?, ?)", corpus_rows()
    )
    connection.execute(f"CREATE TABLE {CORPUS} AS SELECT * FROM lake.main.int_std_records")
    try:
        yield connection
    finally:
        connection.close()


def train_with_spy(
    connection: duckdb.DuckDBPyConnection, cfg: Config, spy: SpyLinker | None = None
) -> tuple[SpyLinker, Any]:
    """One `train_model` against a spy, returning the spy and the result."""
    linker = SpyLinker() if spy is None else spy
    result = train_model(
        connection,
        cfg,
        CORPUS,
        model_version=MODEL_VERSION,
        tf_snapshot_id="ts-0001",
        linker=linker,
    )
    return linker, result


def test_declaration_matches_spec_s4_3_2() -> None:
    """The declared sequence is DesignDoc.md's, statement for statement.

    S4.3.2 says "the call sequence is fixed, and every argument comes from the
    `training:` block". Both halves are checked here: the method paths and their order
    against the fenced block, and that every keyword's value is a `cfg.` reference
    rather than a literal — a literal in that block would be a tunable with no
    `config_hash` covering it.
    """
    assert TRAIN_CALL_SEQUENCE == spec_call_sequence()

    # The EM row is the one with structure, so it is named rather than left to the
    # equality above: one session per entry, repeated over the config's own list.
    em = TRAIN_CALL_SEQUENCE[-1]
    assert em.method_path == "training.estimate_parameters_using_expectation_maximisation"
    assert em.repeat_over == "training.em_blocking_rules"
    assert em.kwargs["blocking_rule"] == EM_RULE
    # Splink 4 namespaces its estimators; a bare `linker.<method>` is the Splink 3 API.
    assert all(spec.method_path.startswith("training.") for spec in TRAIN_CALL_SEQUENCE)


def test_call_sequence_is_exact_and_ordered(corpus: duckdb.DuckDBPyConnection, cfg: Config) -> None:
    """AC1: the calls `train_model` issued are the rendered sequence, and only those."""
    assert render_call_sequence(cfg) == EXPECTED_CALLS

    spy, _ = train_with_spy(corpus, cfg)

    # Full equality rather than a filtered comparison: the spy answers to any
    # attribute, so an extra Splink call would appear here rather than vanish. The one
    # call beyond the sequence is the read-back of what it fitted.
    assert spy.calls == [*EXPECTED_CALLS, TrainCall(SETTINGS_READ_PATH, {})]


def test_estimate_u_is_seeded_from_config(corpus: duckdb.DuckDBPyConnection, cfg: Config) -> None:
    """AC2: `seed` and `max_pairs` are the config's, and `seed` is never left unset.

    Splink's default is `seed=None`, which samples differently on every call. That is
    M8's second half: the model still fits, and the settings JSON differs run to run
    for a reason no test that only checks "did it train" can see.
    """
    spy, _ = train_with_spy(corpus, cfg)

    (sampling,) = [
        call for call in spy.calls if call.method_path.endswith("estimate_u_using_random_sampling")
    ]
    assert sampling.kwargs == {"max_pairs": cfg.training.u_max_pairs, "seed": cfg.training.u_seed}
    assert sampling.kwargs["seed"] is not None
    assert sampling.kwargs == {"max_pairs": 1000000, "seed": 20260101}

    # The value is read from the document, not pinned in the module: a different seed
    # in the config is a different seed on the call.
    doc = document()
    doc["training"]["u_seed"] = 7
    other, _ = train_with_spy(corpus, Config.model_validate(doc))
    (resampled,) = [
        call
        for call in other.calls
        if call.method_path.endswith("estimate_u_using_random_sampling")
    ]
    assert resampled.kwargs["seed"] == 7


def test_one_em_session_per_blocking_rule_in_order(
    corpus: duckdb.DuckDBPyConnection, cfg: Config
) -> None:
    """AC3: one EM session per entry, in config order, each with the configured fix.

    Order is asserted by reordering the config rather than by reading the default
    order twice: a sequence built from a set, or sorted anywhere on the way, would
    satisfy the first list and fail this one.
    """
    doc = document()
    reordered = list(reversed(doc["training"]["em_blocking_rules"]))
    doc["training"]["em_blocking_rules"] = reordered

    spy, _ = train_with_spy(corpus, Config.model_validate(doc))

    sessions = [
        call
        for call in spy.calls
        if call.method_path.endswith("estimate_parameters_using_expectation_maximisation")
    ]
    assert [call.kwargs["blocking_rule"] for call in sessions] == reordered
    assert all(call.kwargs["fix_u_probabilities"] is True for call in sessions)

    # A third rule is a third session, in the position the config puts it.
    doc["training"]["em_blocking_rules"] = [*reordered, "l.phone_e164 = r.phone_e164"]
    third, _ = train_with_spy(corpus, Config.model_validate(doc))
    assert [
        call.kwargs["blocking_rule"]
        for call in third.calls
        if call.method_path.endswith("estimate_parameters_using_expectation_maximisation")
    ] == [*reordered, "l.phone_e164 = r.phone_e164"]

    # `fix_u_probabilities` comes from the config too, and its Splink default is True,
    # so only the False case shows the argument is being passed at all.
    doc["training"]["em"]["fix_u_probabilities"] = False
    unfixed, _ = train_with_spy(corpus, Config.model_validate(doc))
    assert all(
        call.kwargs["fix_u_probabilities"] is False
        for call in unfixed.calls
        if call.method_path.endswith("estimate_parameters_using_expectation_maximisation")
    )


def test_training_block_persisted_verbatim(corpus: duckdb.DuckDBPyConnection, cfg: Config) -> None:
    """AC5: `metrics['training']` is the config's `training:` block, key for key.

    S4.3.2 persists the whole block into `model_registry.metrics`, so the test is
    two-directional: no key the block has may be dropped, and no key it does not have
    may be added. A metrics payload assembled from the fields `train_model` happens to
    read would pass a one-directional check and silently lose every future tunable.
    """
    _, result = train_with_spy(corpus, cfg)

    block = cfg.training.model_dump()
    assert result.metrics["training"] == block
    assert result.training == block
    assert set(result.metrics["training"]) == set(block)
    assert set(block) == {
        "deterministic_rules",
        "recall",
        "u_max_pairs",
        "u_seed",
        "em_blocking_rules",
        "em",
    }
    assert result.metrics["training"]["em"] == {"fix_u_probabilities": True}

    # The rest of the payload is the fit, not the config, and the TF key the frozen
    # rows were written under is part of it (S4.3.2 step 3).
    assert result.metrics["em_sessions"] == len(cfg.training.em_blocking_rules)
    assert result.metrics["probability_two_random_records_match"] == 0.0004
    assert result.metrics["tf_snapshot_id"] == result.tf_snapshot_id == "ts-0001"
    assert result.metrics["tf_columns"] == list(tf_columns(cfg))
    assert result.tf_rows == result.metrics["tf_rows"] > 0


def test_linker_uses_splink_scratch_and_pinned_threads(
    corpus: duckdb.DuckDBPyConnection, cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC6: the API comes from `splink_api`, and `train.py` cannot build its own.

    `output_schema` is not readable off the returned object; what it *does* is issue
    `SET schema`, so the search path is the observable form of the claim — and the
    thread count is pinned to one more than the connection currently reports, so a
    `build_training_linker` that never called `splink_api` could not pass by accident.
    """
    threads = int(scalar(corpus, "SELECT current_setting('threads')")) + 1
    monkeypatch.setenv("ER_DUCKDB_THREADS", str(threads))

    linker = build_training_linker(corpus, cfg, CORPUS)

    assert linker._db_api._con is corpus, "the linker must train on the connection it was given"
    assert scalar(corpus, "SELECT current_schema()") == SPLINK_SCRATCH_SCHEMA
    assert int(scalar(corpus, "SELECT current_setting('threads')")) == threads
    assert int(os.environ["ER_DUCKDB_THREADS"]) == threads

    # The scratch schema lives in the in-memory primary database, never in the lake:
    # `SET schema` names a schema, not a catalog, so with DuckLake as the default
    # catalog every EM intermediate would be committed as a lake snapshot (M17).
    owners = {
        str(name)
        for (name,) in corpus.execute(
            "SELECT database_name FROM duckdb_schemas() WHERE schema_name = ?",
            [SPLINK_SCRATCH_SCHEMA],
        ).fetchall()
    }
    assert owners == {"memory"}

    assert not call_sites(TRAIN_MODULE, DUCKDB_API), (
        f"{TRAIN_MODULE.name} constructs a {DUCKDB_API} of its own; S4.0b confines that to "
        f"er.matching.api.splink_api, and a second construction site is a second chance to "
        f"omit output_schema and put every Splink intermediate in the lake"
    )


def test_repeat_training_is_bit_identical(corpus: duckdb.DuckDBPyConnection, cfg: Config) -> None:
    """AC7: two real trainings over one corpus produce the same settings document.

    The only test here that runs Splink rather than a spy, and the only one that can
    fail for the reason M8 names: the seeded u estimate is what makes the second run's
    sample the first run's sample, and the byte equality of the artifact is what
    T-TRAIN-1 ultimately rests on.
    """
    first = train_model(corpus, cfg, CORPUS, model_version=MODEL_VERSION)
    second = train_model(corpus, cfg, CORPUS, model_version=MODEL_VERSION)

    assert first.settings_json == second.settings_json
    assert first.settings == second.settings
    # A settings document that fitted nothing would also be equal to itself.
    assert first.settings["comparisons"], "the fitted model has no comparisons"
    assert isinstance(first.settings["probability_two_random_records_match"], float)
    # The equality above is a claim about the fitted model, so the keys that are not
    # fitted have to be gone -- and gone because they *would* have differed, which is
    # what two linkers over one corpus show.
    for key in NON_FITTED_SETTINGS_KEYS:
        assert key not in first.settings
    uids = {
        build_training_linker(corpus, cfg, CORPUS).misc.save_model_to_json()["linker_uid"]
        for _ in range(2)
    }
    assert len(uids) == 2, (
        "Splink no longer mints a per-linker uid; NON_FITTED_SETTINGS_KEYS is dropping a "
        "key that carries model state"
    )

    # Each run mints its own TF snapshot, because D4 makes `er train` a mint site and
    # the frozen rows are the corpus the model was fitted against.
    assert first.tf_snapshot_id != second.tf_snapshot_id
    assert first.tf_rows == second.tf_rows > 0
