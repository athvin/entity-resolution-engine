"""Integration-layer fixtures (DesignDoc.md S8.1).

The session harness — `lake_ns`, `er_env`, `object_store`, `catalog`,
`lake_conn` — lives one directory up in `tests/conftest.py`, because the
namespace contract is a property of the *session* and not of this layer, and
because the unit layer must be able to import the same module on a bare runner
without requesting any of it.

This module is the seam the later isolation tickets extend, and it is
deliberately empty until they land:

* the function-scoped fixture that `DELETE`s from every `ddl.py`-owned relation,
  drops the dbt-owned ones and reloads the scenario fixture, together with
  `--keep-lake` and the `sub_namespace` factory T-INC-1 needs (ER-026);
* the T-INV-1 autouse finalizer (M3).

It exists now so those tickets add a fixture to a file that is already collected
rather than introducing a new collection point along with their behaviour.
"""

from __future__ import annotations
