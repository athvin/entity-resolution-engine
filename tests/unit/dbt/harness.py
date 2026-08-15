"""Render a dbt macro through Jinja and run the SQL it produces (DesignDoc.md S8.1).

S8.1's Unit row asks for "normalizer macros against a bare in-process DuckDB", and
that row is the whole design of this module. The unit layer runs on a bare runner:
no Postgres catalog, no object store, no `ATTACH`, and no `dbt` subprocess -- S8.1
forbids even `dbt compile` on the static layer for exactly that reason. So a macro
is exercised the only way that is left: its Jinja source is rendered here, and the
SQL fragment that falls out is executed against `duckdb.connect()` with no
arguments.

Three consequences follow, and each is a property a test can assert rather than a
convention a reader has to trust:

* :meth:`MacroHarness.execute` REFUSES a statement containing ``ATTACH``. A macro
  that reached for a warehouse would fail loudly here instead of quietly turning
  the unit suite into an integration suite that only passes with Docker running.
* ``var()`` reads the dict the caller supplies -- in practice the payload
  ``src/er/dbt_runner.py::render_dbt_vars`` builds from the S6 document, which is
  what makes "the macro reads its settings from the `standardization` var"
  checkable by changing the var and re-rendering.
* ``ref()``, ``source()`` and ``seed()`` resolve against registrations rather than
  a manifest, and raise on an unregistered name. A macro under test names the
  relations it needs and gets them; one that names something nobody registered is
  a test bug, and silently inventing a relation name would hide it.

Every macro under ``dbt/macros/`` is compiled into ONE Jinja module, so a macro can
call another by bare name (`email_norm` calls `lowercase_trim`) exactly as it does
under real dbt.
"""

from __future__ import annotations

import re
from collections.abc import Iterator, Mapping, Sequence
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import duckdb
from jinja2 import Environment, StrictUndefined

__all__ = [
    "INPUT_COLUMN",
    "MACRO_ROOT",
    "RESULT_COLUMN",
    "MacroHarness",
    "split_projections",
]

#: Repo root, from ``tests/unit/dbt/harness.py``.
REPO_ROOT = Path(__file__).resolve().parents[3]

#: The macro tree S3 places at ``dbt/macros/``; every ``*.sql`` under it is loaded.
MACRO_ROOT = REPO_ROOT / "dbt" / "macros"

#: The column :meth:`MacroHarness.eval_macro` binds its input to. A macro takes a
#: COLUMN expression, not a literal, so the value has to arrive as a column or the
#: NULL semantics under test would not be the ones the pipeline sees.
INPUT_COLUMN = "col"

#: The alias :meth:`MacroHarness.eval_macro` gives a single-projection macro.
#: Multi-projection macros (`email_norm`) keep their own aliases -- those aliases
#: are part of the S5 column contract and must not be rewritten by the harness.
RESULT_COLUMN = "value"

# The guard behind AC1. Word-bounded so `duckdb_databases()` and prose are not hits.
_ATTACH_RE = re.compile(r"\battach\b", re.IGNORECASE)

# Distinguishes "no default supplied" from "default is None", which `var()` needs:
# dbt's own `var(name)` raises on a missing var, and mimicking that is what keeps a
# macro from silently standardizing under settings nobody passed.
_MISSING = object()


def split_projections(sql: str) -> tuple[str, ...]:
    """Split a rendered SELECT-list fragment on its TOP-LEVEL commas.

    Needed because a macro's projection count is part of its contract: S4.2 says
    `email_norm` emits `email` and `email_valid`, and `lowercase_trim` emits one
    bare expression. Commas inside `regexp_replace(...)` or inside a quoted
    placeholder list are not separators, so a naive ``sql.split(",")`` would report
    a two-projection macro as five.
    """
    parts: list[str] = []
    depth = 0
    quoted = False
    start = 0
    index = 0
    while index < len(sql):
        char = sql[index]
        if quoted:
            if char == "'":
                # '' is an escaped quote inside a literal, not the end of one.
                if sql[index + 1 : index + 2] == "'":
                    index += 2
                    continue
                quoted = False
        elif char == "'":
            quoted = True
        elif char in "([":
            depth += 1
        elif char in ")]":
            depth -= 1
        elif char == "," and depth == 0:
            parts.append(sql[start:index].strip())
            start = index + 1
        index += 1
    parts.append(sql[start:].strip())
    return tuple(part for part in parts if part)


class MacroHarness:
    """A dbt macro tree, rendered and executed with no warehouse behind it."""

    def __init__(
        self,
        vars: Mapping[str, Any] | None = None,
        macro_root: Path = MACRO_ROOT,
    ) -> None:
        self._vars: dict[str, Any] = dict(vars or {})
        self._macro_root = macro_root
        self._relations: dict[str, str] = {}
        self._seeds: dict[str, str] = {}
        self._executed: list[str] = []
        # No path, no extensions, no settings: the connection a bare runner can
        # open and the one S8.1's Unit row describes.
        self._connection = duckdb.connect()
        self._environment = self._build_environment()
        self._module = self._build_module()
        # dbt puts every project macro in one global namespace; mirroring the
        # module's exports back into the environment is what lets an ad-hoc
        # fragment -- a model body, in a later ticket -- call them by bare name.
        self._environment.globals.update(
            {name: getattr(self._module, name) for name in self.macro_names}
        )

    # -- introspection -----------------------------------------------------

    @property
    def executed_sql(self) -> tuple[str, ...]:
        """Every statement this harness has run, in order."""
        return tuple(self._executed)

    @property
    def macro_names(self) -> tuple[str, ...]:
        """The macros the tree defines, sorted."""
        return tuple(sorted(name for name in dir(self._module) if not name.startswith("_")))

    # -- the dbt call surface the macros see -------------------------------

    def register_relation(self, key: str, relation: str) -> str:
        """Make `ref('<key>')` -- or `source('<schema>','<name>')` as `schema.name` -- resolve."""
        self._relations[key] = relation
        return relation

    def register_seed(self, name: str, rows: Sequence[Mapping[str, Any]]) -> str:
        """Materialize a seed as a real table and make `seed()`/`ref()` resolve to it.

        `nickname_variants` (S3) is the seed v1 actually has, and ER-039's
        `name_variants` joins it, so a seed has to be a table with the right column
        names rather than a name the harness hands back unbacked.
        """
        materialized = list(rows)
        if not materialized:
            raise ValueError(f"seed {name!r}: no rows; a seed with no columns cannot be typed")
        columns = list(materialized[0])
        for row in materialized:
            if list(row) != columns:
                raise ValueError(f"seed {name!r}: every row must carry the same columns {columns}")
        table = f"seed_{name}"
        placeholders = ", ".join("?" for _ in columns)
        tuples = ", ".join(f"({placeholders})" for _ in materialized)
        quoted = ", ".join(f'"{column}"' for column in columns)
        parameters = [row[column] for row in materialized for column in columns]
        self.execute(
            f'create or replace table "{table}" as '
            f"select * from (values {tuples}) as seed_rows({quoted})",
            parameters,
        )
        self._seeds[name] = table
        # A dbt seed is referenced with `ref()` as well as by name, so both resolve.
        self._relations.setdefault(name, table)
        return table

    # -- rendering and execution -------------------------------------------

    def render(self, source: str, vars: Mapping[str, Any] | None = None) -> str:
        """Render an ad-hoc Jinja fragment against the same globals a macro sees.

        The macros are reachable from it by name, which is how a model body would
        reach them, and so are `var()`, `ref()`, `source()` and `seed()` -- the
        only way to exercise a stub that no shipped macro calls yet.
        """
        with self._overlay(vars):
            return self._environment.from_string(source).render().strip()

    def render_macro(self, name: str, *args: Any, vars: Mapping[str, Any] | None = None) -> str:
        """Render ``name(*args)`` and return the SQL fragment, stripped."""
        macro = getattr(self._module, name, None)
        if macro is None:
            raise AttributeError(
                f"no macro named {name!r} under {self._macro_root}; "
                f"the tree defines {', '.join(self.macro_names) or '(nothing)'}"
            )
        with self._overlay(vars):
            return str(macro(*args)).strip()

    def eval_macro(
        self,
        name: str,
        value: Any,
        *args: Any,
        vars: Mapping[str, Any] | None = None,
    ) -> list[dict[str, Any]]:
        """Render ``name`` over one input value and return the rows it produces.

        The value is bound as :data:`INPUT_COLUMN` -- cast to VARCHAR so that
        ``None`` arrives as a typed SQL NULL rather than an untyped parameter the
        planner has to guess at -- and the macro is rendered against that column
        name. A single-projection macro is aliased :data:`RESULT_COLUMN`; a macro
        that emits its own aliases keeps them.
        """
        rendered = self.render_macro(name, INPUT_COLUMN, *args, vars=vars)
        projections = split_projections(rendered)
        selected = rendered if len(projections) > 1 else f"{rendered} as {RESULT_COLUMN}"
        cursor = self.execute(
            f"select {selected} from (select cast(? as varchar) as {INPUT_COLUMN}) as input",
            [value],
        )
        columns = [column[0] for column in cursor.description or ()]
        return [dict(zip(columns, row, strict=True)) for row in cursor.fetchall()]

    def execute(self, sql: str, parameters: Sequence[Any] | None = None) -> Any:
        """Run one statement against the in-process connection, recording it.

        Refuses ``ATTACH``: the unit layer has no catalog and no object store to
        attach, and a harness that quietly opened one would make the suite pass
        only where Docker is running (S8.1).
        """
        if _ATTACH_RE.search(sql):
            raise RuntimeError(
                "the unit macro harness never attaches a warehouse; "
                "a test that needs the lake belongs in tests/integration (S8.1)"
            )
        self._executed.append(sql)
        return self._connection.execute(sql, list(parameters) if parameters else [])

    def close(self) -> None:
        self._connection.close()

    # -- internals ---------------------------------------------------------

    def _build_environment(self) -> Environment:
        environment = Environment(
            undefined=StrictUndefined,
            autoescape=False,  # SQL, not HTML; escaping here would corrupt every literal.
            keep_trailing_newline=True,
        )
        environment.globals.update(
            var=self._var,
            ref=self._ref,
            source=self._source,
            seed=self._seed,
        )
        return environment

    def _build_module(self) -> Any:
        sources = sorted(self._macro_root.rglob("*.sql"))
        if not sources:
            raise FileNotFoundError(f"no macros under {self._macro_root}")
        # One template for the whole tree, so cross-macro calls resolve by bare
        # name the way they do under dbt's global macro namespace.
        combined = "\n".join(path.read_text(encoding="utf-8") for path in sources)
        return self._environment.from_string(combined).make_module()

    @contextmanager
    def _overlay(self, vars: Mapping[str, Any] | None) -> Iterator[None]:
        """Apply per-render var overrides, then put the base payload back."""
        if not vars:
            yield
            return
        saved = dict(self._vars)
        self._vars.update(vars)
        try:
            yield
        finally:
            self._vars = saved

    def _var(self, name: str, default: Any = _MISSING) -> Any:
        if name in self._vars:
            return self._vars[name]
        if default is not _MISSING:
            return default
        raise KeyError(
            f"var({name!r}) is not set; the payload comes from "
            f"src/er/dbt_runner.py::render_dbt_vars (S6)"
        )

    def _ref(self, name: str) -> str:
        return self._resolve(self._relations, name, f"ref({name!r})")

    def _source(self, schema: str, name: str) -> str:
        key = f"{schema}.{name}"
        return self._resolve(self._relations, key, f"source({schema!r}, {name!r})")

    def _seed(self, name: str) -> str:
        return self._resolve(self._seeds, name, f"seed({name!r})")

    @staticmethod
    def _resolve(registry: Mapping[str, str], key: str, call: str) -> str:
        try:
            return registry[key]
        except KeyError:
            known = ", ".join(sorted(registry)) or "(nothing)"
            raise KeyError(f"{call} is not registered; the harness knows {known}") from None
