"""Shared test helpers (S8.2.1).

A package rather than a loose directory so ``helpers.compare`` names the same
module under pytest and under ``mypy --strict tests/helpers``: mypy derives a
module name by walking up while ``__init__.py`` exists, and ``tests/`` has none,
so this file is what fixes the name at ``helpers.*`` for both tools. The pytest
side is ``pythonpath = ["tests"]`` in ``pyproject.toml``.

Nothing is re-exported here. ``compare.py`` exposes exactly the three functions
S8.2.1 pins and no others, and a package-level convenience import would be a
fourth public spelling of them.
"""
