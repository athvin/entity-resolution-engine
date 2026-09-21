# Contributing

Use Python 3.12 and the pinned uv version in [the version table](DesignDoc.md#s2-1).
Docker Compose is required for integration tests and benchmarks.

```sh
uv sync --frozen
make check
```

`make check` runs specification validation, Ruff, strict mypy, workflow lint,
fixture validation, dbt dependency installation/parsing, and unit tests. It needs
no lake. The first dbt dependency installation needs network access. Individual
targets are `spec`, `lint`, `types`, `workflows`, `fixtures`, `dbt` and `unit`.

## Integration tests

```sh
make integration
# Or select the tests affected by a change:
COMPOSE_PROJECT_NAME=er-integration bash scripts/ci/itest.sh \
  tests/integration/test_reconcile_apply.py -q
```

The runner builds the image and resets its Compose project before and after the
suite. Use a dedicated project name; the commands above use `er-integration`.
`make check-all` runs the local checks followed by the non-slow integration suite.
Run slow model-regeneration checks separately when changing the trained fixture.

CI runs static and unit checks, then 32 isolated integration shards. To reproduce
one shard, add `--integration-shard-index 0 --integration-shard-count 32` and
`-m 'not slow'` to the integration runner's pytest arguments. JUnit and dbt
diagnostics are written under `artifacts/`.

## Making a change

Use a branch and a pull request. Explain the behavior changed and the validation
performed. Tests should exercise observable behavior, including relevant merge,
split, deletion or incremental cases when changing resolution logic.

Keep dependency pins in `pyproject.toml`, `uv.lock`, `src/er/versions.py` and the
specification consistent. Keep Python-owned relation definitions in
`src/er/lake/model.py` / `ddl.py`, and dbt-owned contracts in their `schema.yml`
files. Update [the reference specification](DesignDoc.md) when a contract changes.

The fixture format is documented in [fixtures/static/FORMAT.md](fixtures/static/FORMAT.md).
Benchmark and profiling changes should follow [performance.md](docs/performance.md)
and [profiling.md](docs/profiling.md).

## Keeping the checkout small

```sh
make clean
```

This removes Python/test caches, dbt build output, downloaded dbt packages and its
machine-local usage file. It preserves the virtual environment and run artifacts.
The next `make check` restores dbt packages.

Remove individual directories under `artifacts/` after extracting any result worth
retaining. Keep durable measurements and methodology in the performance guide;
raw logs, exports, generated corpora and temporary scripts stay out of Git. Keep
additional Git worktrees outside `artifacts/` so run cleanup cannot erase a checkout.
