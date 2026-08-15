"""The macro harness contract (DesignDoc.md S8.1; S4.2).

Every later macro ticket is written against this harness, so what it guarantees is
asserted here rather than assumed there. The load-bearing guarantee is negative:
a macro test opens NO warehouse connection and spawns NO dbt subprocess. S8.1 puts
`dbt compile --target mem` in the Unit layer and forbids it on the Static layer
because `compile` needs a connection; a harness that quietly attached would move
the whole normalizer suite into Integration without anyone noticing until CI ran
on a runner with no Docker.
"""

from __future__ import annotations

import subprocess
from typing import Any

import pytest
from harness import MacroHarness

# The macros S4.2 assigns to this ticket. `dbt/macros/std/` gains more of them in
# ER-038/039/040, so this is a subset check rather than an equality one.
SHIPPED_MACROS = ("NULL_SENTINELS", "email_norm", "lowercase_trim", "null_semantics")

# Everything a bare `duckdb.connect()` should have open. A fourth name here means
# something attached.
IN_PROCESS_DATABASES = {"memory", "system", "temp"}

# What `subprocess` exposes that could start dbt. Patched to refuse, so "no dbt
# subprocess" is enforced during the test rather than inspected after it.
SPAWNERS = ("run", "Popen", "call", "check_call", "check_output")


def test_renders_and_executes_a_macro_in_process(harness: MacroHarness) -> None:
    """A macro under `dbt/macros/` renders to SQL, and that SQL runs and returns rows."""
    assert set(SHIPPED_MACROS) <= set(harness.macro_names)

    rendered = harness.render_macro("lowercase_trim", "email_address")
    # The rendered fragment is SQL over the column it was handed, not a value.
    assert "email_address" in rendered
    assert "nfc_normalize" in rendered

    assert harness.eval_macro("lowercase_trim", "  Ada ") == [{"value": "ada"}]
    assert any("nfc_normalize" in statement for statement in harness.executed_sql)


def test_harness_uses_no_warehouse_connection(
    dbt_vars: dict[str, Any], monkeypatch: pytest.MonkeyPatch
) -> None:
    """Constructing and using a harness attaches nothing and spawns nothing."""

    def refuse(*args: object, **kwargs: object) -> object:
        raise AssertionError(f"the unit macro harness spawned a subprocess: {args!r}")

    for name in SPAWNERS:
        monkeypatch.setattr(subprocess, name, refuse)

    # Built AFTER the patch, so construction is covered too, not just use.
    harness = MacroHarness(vars=dbt_vars)
    try:
        harness.eval_macro("email_norm", "ada@example.com")

        databases = {
            row[0]
            for row in harness.execute("select database_name from duckdb_databases()").fetchall()
        }
        assert databases <= IN_PROCESS_DATABASES

        assert not any("attach" in statement.lower() for statement in harness.executed_sql)

        with pytest.raises(RuntimeError, match="never attaches"):
            harness.execute("attach 'ducklake:postgres:dsn' as lake")
    finally:
        harness.close()


def test_seed_and_ref_stubs_resolve(dbt_vars: dict[str, Any]) -> None:
    """`ref()`, `source()` and `seed()` resolve registrations, and a seed is a real table."""
    harness = MacroHarness(vars=dbt_vars)
    try:
        harness.register_relation("int_std_records", "main.int_std_records")
        harness.register_relation("lake.raw_records", "lake.main.raw_records")
        table = harness.register_seed(
            "nickname_variants",
            [
                {"name": "bob", "variant": "robert"},
                {"name": "liz", "variant": "elizabeth"},
            ],
        )

        assert harness.render("{{ ref('int_std_records') }}") == "main.int_std_records"
        assert harness.render("{{ source('lake', 'raw_records') }}") == "lake.main.raw_records"
        assert harness.render("{{ seed('nickname_variants') }}") == table
        # A dbt seed is reachable through `ref()` as well, which is how S4.2's
        # `name_norm` will join `nickname_variants` in ER-039.
        assert harness.render("{{ ref('nickname_variants') }}") == table

        rows = harness.execute(f'select name, variant from "{table}" order by name').fetchall()
        assert rows == [("bob", "robert"), ("liz", "elizabeth")]

        # An unregistered relation is a test bug, not a name to invent.
        with pytest.raises(KeyError, match="not registered"):
            harness.render("{{ ref('golden_records') }}")
    finally:
        harness.close()


def test_var_is_the_only_source_of_standardization_settings(harness: MacroHarness) -> None:
    """A var nobody passed raises rather than defaulting (S6: one source of truth)."""
    assert harness.render("{{ var('standardization')['phone_default_region'] }}") == "US"
    with pytest.raises(KeyError, match="render_dbt_vars"):
        harness.render("{{ var('nonexistent_block') }}")
