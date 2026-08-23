"""T-INV-1 as a collectible node id (S8.3).

S8.3 gives T-INV-1 a `pytest node id` of
`tests/integration/test_invariants.py::test_membership_equals_connected_components`,
and the S8.3 table is machine-read: a row whose node id does not resolve is a row
nobody can run. The invariant itself is an autouse finalizer in
`tests/integration/conftest.py` — it has to be, because it must run after *every*
scenario — so this module exists to make that node id real in two ways at once.

* **It resolves.** The node id below collects and runs under plain `pytest`, so the
  S8.3 row is executable rather than notional.
* **It is where finalizer failures are reported.** The conftest emits a synthetic
  report under this exact node id, so a scenario that passes while the invariant fails
  shows up in the junit XML as a T-INV-1 failure rather than as a teardown error on
  whichever scenario happened to run last.

The body asserts the same five clauses the finalizer does, through the same helper.
That is the point: one implementation in `tests/helpers/invariants.py`, two call sites,
so the node id and the finalizer can never come to mean different things.
"""

from __future__ import annotations

import duckdb
from helpers.invariants import assert_membership_equals_components


def test_membership_equals_connected_components(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """T-INV-1's five clauses over a freshly initialised lake (S8.3).

    On today's board `entity_membership` is empty — the reconcile stage that writes it
    is ER-074 — so clauses 1 to 3 hold vacuously while clauses 4 and 5 do real work:
    every stored pair is canonical, and nothing of Splink's is in the lake. The helper
    passes rather than skips on an empty membership, which is what keeps this node
    meaningful for the whole of M4 instead of being a skip somebody has to remember to
    remove.
    """
    assert_membership_equals_components(initialised_lake)
