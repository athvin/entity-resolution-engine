"""The seeded synthetic corpus generator (DesignDoc.md S10.1).

`fixtures/` is data, not a distribution, so this package is not installed and is
not importable as `er.*`. It carries an `__init__.py` all the same: without one,
`personas.py` and the `corruptions.py` / `emit.py` that follow it would each be a
top-level module named after its own file, and two of the three would collide with
something in a repository this size sooner or later. One package name -- `generator`
-- also gives `mypy --strict fixtures/generator` a single module tree to check.
"""
