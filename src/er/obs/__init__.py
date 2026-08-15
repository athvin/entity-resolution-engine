"""Run metadata and structured logging (DesignDoc.md S4 preamble, S5.2).

`run_id` is referenced by six relations; this package is what gives it a referent.
Every CLI invocation opens a :class:`~er.obs.runctx.RunContext`, every stage inside
it writes exactly one `run_stages` row and emits exactly one JSON line on stderr,
and the touched set the S4.6 marts join lives here too.

The four modules split along the lines S5.2 draws, not along convenience:

* :mod:`er.obs.counters` — the closed set of promoted counter *columns* and the
  free-form `counters` JSON payload, plus the completeness rule that binds them.
* :mod:`er.obs.logging` — the one emitter of the S5.2 stderr record.
* :mod:`er.obs.runctx` — the `runs` and `run_stages` rows, their snapshot ranges
  and their terminal status on every exit path including failure.
* :mod:`er.obs.touched` — the `er_touched_entities` accessor.

Not drawn in the S3 tree: S3's three normative layout rules are about where stage
code, dbt models and fixtures live, and none of them is affected by a package that
owns the observability layer instead of scattering it across every stage module.
"""

from __future__ import annotations
