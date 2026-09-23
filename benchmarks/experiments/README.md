# Unpromoted survivorship alternatives

These macros are screening candidates, outside dbt's production macro directory.
`survivorship_top_two.sql` replaces complete ranking with an `arg_min(..., 2)`
aggregate and an encoded sort key. `survivorship_lead.sql` obtains the runner-up
through `lead` in the same window used to select the winner.

Both preserve the winner/runner-up comparison, including null winners, ties,
frequency rules and the terminal record-key ordering. The existing production
window remains in use: the aggregate was slower in the million-row screen, and
the `lead` variant did not show a consistent gain for full and touched subsets.

Reproduce the screen with an unchanged baseline macro:

```sh
git show 00a65e4:dbt/macros/survivorship/survivorship.sql > /tmp/er-baseline-survivorship.sql
uv run python benchmarks/tuning_kernels.py \
  --baseline-macro /tmp/er-baseline-survivorship.sql \
  --out artifacts/performance/tuning-kernels.json
```

This also compares bounded upload sizes and current-edge table/view reuse. Kernel
timings choose which candidates warrant full pipeline trials; they are not claims
about full-reload or incremental end-to-end performance.
