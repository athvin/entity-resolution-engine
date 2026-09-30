"""`stg_records` renders one union arm per S6 source — the S4.2 claim, executable.

The unified staging model is what makes a new source a config change alone: no
model file accompanies `sources.<name>`, so the ONLY place the source list can
shape the compiled SQL is `var('sources')`. These tests render the real model
body through the macro harness with payloads a real `render_dbt_vars` call would
carry — a fourth source, a single source, an empty document — and assert on the
SQL that falls out, because the arm count and the per-arm batch predicate are
the S4.2 contract, not an implementation detail.

Rendered, not executed: running the projection needs the lake and belongs to
`tests/integration/scenarios/test_staging.py` (S8.1).
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest
from harness import MacroHarness

REPO_ROOT = Path(__file__).resolve().parents[3]
MODEL = (REPO_ROOT / "dbt" / "models" / "staging" / "stg_records.sql").read_text(encoding="utf-8")

#: A spec shaped exactly as `render_dbt_vars` ships one (SourceSpec.model_dump()).
COLUMNS = {
    "given_name": "first",
    "family_name": "last",
    "email": "mail",
    "phone": "tel",
    "address_line": "street",
    "addr_city": "city",
    "addr_region": "region",
    "addr_postal": "postal",
    "birth_date": "born",
}

STANDARDIZATION = {
    "email_strip_plus_addressing": False,
    "email_placeholders": [],
    "phone_default_region": "US",
}


class CompilerError(Exception):
    """What the `exceptions.raise_compiler_error` stub raises."""


class _Exceptions:
    @staticmethod
    def raise_compiler_error(message: str) -> None:
        raise CompilerError(message)


def spec(rank: int) -> dict[str, object]:
    return {
        "adapter": "csv",
        "priority_rank": rank,
        "record_id_column": "id",
        "updated_at_column": "updated",
        "date_format": "%Y-%m-%d",
        "columns": dict(COLUMNS),
    }


def render(sources: dict[str, object], *, incremental: bool = True) -> str:
    harness = MacroHarness(
        vars={"sources": sources, "std_version": "1", "standardization": STANDARDIZATION}
    )
    try:
        harness.register_relation("lake.raw_records", "lake.main.raw_records")
        harness.register_relation("lake.nickname_variants", "lake.main.nickname_variants")
        return harness.render(
            MODEL,
            context={
                "config": lambda **_: "",
                "is_incremental": lambda: incremental,
                "this": "lake.main.stg_records",
                "exceptions": _Exceptions(),
            },
        )
    finally:
        harness.close()


def arms(rendered: str) -> list[str]:
    """The arms' outer `where source_system = '<name>'` filters, in rendered order.

    The batch predicate's subquery carries the same filter but closes a paren
    right after it, which is what the lookahead excludes.
    """
    return re.findall(r"where source_system = '(\w+)'(?!\))", rendered)


def separators(rendered: str) -> int:
    """Top-level arm separators: `union all` alone on a line, not one a macro
    emits inside an expression."""
    return len(re.findall(r"^union all$", rendered, re.MULTILINE))


def test_four_sources_render_four_arms_in_sorted_order() -> None:
    rendered = render(
        {"crm": spec(1), "billing": spec(2), "webforms": spec(3), "partnerapp": spec(4)}
    )
    assert arms(rendered) == ["billing", "crm", "partnerapp", "webforms"]
    assert separators(rendered) == 3
    # One batch predicate per arm, each consulting only its own source's batches.
    assert rendered.count("ingest_batch_id not in") == 4
    for source in ("billing", "crm", "partnerapp", "webforms"):
        assert f"from lake.main.stg_records\n      where source_system = '{source}'" in rendered


def test_single_source_renders_no_union() -> None:
    rendered = render({"only": spec(1)})
    assert arms(rendered) == ["only"]
    assert separators(rendered) == 0


def test_first_build_renders_no_batch_predicate() -> None:
    rendered = render({"crm": spec(1), "billing": spec(2)}, incremental=False)
    assert arms(rendered) == ["billing", "crm"]
    assert "ingest_batch_id not in" not in rendered


def test_empty_sources_refuse_to_compile() -> None:
    with pytest.raises(CompilerError, match="at least one source"):
        render({})


def test_column_mapping_is_read_per_arm() -> None:
    """A rename in ONE source's mapping moves exactly that arm's accessor."""
    renamed = spec(2)
    renamed["columns"] = {**COLUMNS, "email": "email_address"}
    rendered = render({"crm": spec(1), "billing": renamed})
    # Sorted arm order puts billing (the renamed mapping) first; the macros
    # expand one accessor several times, so the claim is per-arm exclusivity,
    # not a count.
    billing_arm, crm_arm = re.split(r"^union all$", rendered, flags=re.MULTILINE)
    assert "payload ->> 'email_address'" in billing_arm
    assert "payload ->> 'mail'" not in billing_arm
    assert "payload ->> 'mail'" in crm_arm
    assert "payload ->> 'email_address'" not in crm_arm
