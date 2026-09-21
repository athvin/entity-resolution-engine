# Operating the engine

Commands below run from the repository root. Training is always explicit;
`run-all` uses an existing active model. For an isolated, automatically cleaned-up
smoke run, use the [README quickstart](../README.md#try-the-complete-pipeline).

## Development lake

Use a dedicated Compose project to keep this lake separate from integration tests:

```sh
mkdir -p artifacts/demo
docker compose -p er-demo -f docker/compose.yaml --profile test build pipeline

erdev() {
  docker compose -p er-demo -f docker/compose.yaml --profile test \
    run --rm pipeline er "$@"
}

erdev init
erdev doctor
```

The Compose dependencies initialize PostgreSQL and the object store. `erdev` uses
`configs/test.yaml` and the container's lake environment. Keep the function in the
same shell for the remaining examples. The lake survives individual `run --rm`
commands; removing the stack destroys this development data.

Generate a usable training corpus, then arrange it as source delivery folders:

```sh
docker compose -p er-demo -f docker/compose.yaml --profile test run --rm pipeline \
  python -m fixtures.generator.cli --personas 400 --records 1000 \
  --batch 50 --seed 42 --out /app/artifacts/demo/corpus

for source in crm billing webforms; do
  mkdir -p "artifacts/demo/base/$source" "artifacts/demo/batch/$source"
  cp "artifacts/demo/corpus/$source.csv" "artifacts/demo/base/$source/records.csv"
  cp "artifacts/demo/corpus/batch/$source.csv" "artifacts/demo/batch/$source/records.csv"
done
```

For your own deliveries, use the same directory layout and the
[source mappings](configuration.md#source-data) for their column names.

## First load

```sh
ER_INITIAL_RUN_ID=$(docker run --rm er-pipeline:ci python -c 'from ulid import ULID; print(ULID())')
for source in crm billing webforms; do
  erdev ingest --run-id "$ER_INITIAL_RUN_ID" \
    --source "$source" --path /app/artifacts/demo/base
done
erdev standardize --run-id "$ER_INITIAL_RUN_ID"
erdev train --run-id "$ER_INITIAL_RUN_ID"
erdev run-all --mode full --skip-ingest --run-id "$ER_INITIAL_RUN_ID"
```

Keep the same run ID across this sequence: reconciliation uses it to find the
ingested records and assembly uses it to find the touched entities.

The final command standardizes any remaining input, scores all candidates,
reconciles entities and assembles golden records. Successful stage summaries go to
stderr; `--json` selects machine-readable stdout. Run metadata lives in `runs` and
`run_stages`.

The main outputs are `lake.main.golden_records`, `golden_lineage`,
`entity_membership` and `entity_events`. To export golden records from the demo:

```sh
docker compose -p er-demo -f docker/compose.yaml --profile test run --rm -T pipeline python - <<'PYTHON'
from er.lake.ducklake import connect

with connect() as connection:
    connection.execute(
        "COPY lake.main.golden_records TO "
        "'/app/artifacts/demo/golden_records.csv' (FORMAT CSV, HEADER)"
    )
PYTHON
```

The CSV appears at `artifacts/demo/golden_records.csv` on the host.

## Incremental deliveries

For a single source, ingest and process it in one command:

```sh
erdev run-all --mode incremental --source crm --path /app/artifacts/demo/batch
```

For a delivery spanning all three sources, use one run ID for the ingests and chain:

```sh
ER_DELIVERY_RUN_ID=$(docker run --rm er-pipeline:ci python -c 'from ulid import ULID; print(ULID())')
for source in crm billing webforms; do
  erdev ingest --run-id "$ER_DELIVERY_RUN_ID" \
    --source "$source" --path /app/artifacts/demo/batch
done
erdev run-all --mode incremental --skip-ingest --run-id "$ER_DELIVERY_RUN_ID"
```

Choose one of these two examples for a delivery. Repeated unchanged input can return
exit `10`, a successful no-op. A shell wrapper running individual commands must
accept both `0` and `10`; `run-all` handles this within its own chain.

Use `ingest --full-refresh-keys` only when the delivery contains the source's
complete current key set. Missing keys are deleted from current membership during
reconciliation; a later ordinary delivery can resurrect them.

## Full rebuilds and correction

After configuration or version changes:

```sh
erdev run-all --mode full --skip-ingest --reason operator
```

An incremental run normally refuses configuration/model drift. `--allow-escalate`
permits `run-all` to promote it to a full run. Supported rebuild reasons are
`operator`, `std_version_bump`, `survivorship_version_bump` and `correction_pass`.

After `erdev train` activates a new model, run the full chain before continuing
incremental processing. To refresh term frequencies and repair incremental drift
without retraining:

```sh
erdev correct
```

Correction runs full matching, reconciliation and assembly against the current
standardized corpus. It does not ingest or standardize new data. Invoke it from
your scheduler at the configured `correction_pass.cadence`; the CLI itself does
not run a scheduler.

## Review and assertions

```sh
erdev review list --status open --limit 100 --json
erdev review resolve --review-id REVIEW_ID --as match --by steward
erdev assert add --a crm:123 --b billing:456 --kind never --by steward
erdev assert remove --assertion-id ASSERTION_ID --by steward
erdev run-all --mode incremental --skip-ingest
```

Replace the sample IDs with real keys. Review resolutions are `match`, `no_match`
and `dismiss`; match decisions persist an `always` assertion and no-match decisions
persist a `never` assertion. `assert add` also accepts `--note`. `assert load --path
FILE` loads an assertion file; see the [assertion contract](../DesignDoc.md#s4-4).
Reconciliation applies assertion changes to the entity graph. Contradictions fail
with the conflicting assertions identified for correction.

## Failures and recovery

| Exit | Meaning | Action |
|---|---|---|
| `0` | Success | Continue |
| `1` | Stage or doctor check failed | Inspect the error and failed check/stage |
| `2` | Invalid arguments, config or environment | Correct the input before rerunning |
| `3` | Unmet precondition or writer lock conflict | Resolve the named prerequisite |
| `10` | Nothing to do | Treat as a successful no-op |

`run_stages.error_class` distinguishes `transient_io`, `lock_conflict`,
`precondition`, `config`, `contradiction`, `non_convergence` and `data` errors.
Only transient I/O and lock conflicts are classified as retryable; the CLI never
retries automatically. Inspect stderr and `error_detail` before restarting.

To resume a failed chain, retain its original config and input paths:

```sh
erdev run-all --mode incremental --source crm --path /app/artifacts/demo/batch \
  --resume FAILED_RUN_ID
```

Use `--skip-ingest` if the original chain skipped ingestion. Resume uses the
recorded mode, run ID and first unfinished stage; the config hash must match.
An already successful run or a run with no unfinished recorded stage cannot resume.

## Maintenance and shutdown

`erdev lake maintain --retain-days 7` compacts data and expires eligible snapshots;
seven days is the default. Maintenance refuses to expire snapshots referenced by
active or failed runs. Resolve failed runs before advancing their retention window.

`erdev lake reset --confirm-tenant test` destroys the selected namespace. The
confirmation must match the configured tenant. `init --force` does not perform a
reset and recreates nothing.

When finished with the disposable demo lake:

```sh
docker compose -p er-demo -f docker/compose.yaml --profile test down -v --remove-orphans
```

Exported host files remain under `artifacts/demo/` until you remove that directory.

## Individual commands

Every command accepts `--config`, `--run-id`, `--json` and `--help` after its name.
The chain can also be run as individual stages under a shared run ID:

| Command | Stage-specific options |
|---|---|
| `init` | `--force` (non-destructive) |
| `doctor` | No additional options |
| `ingest` | Required `--source`, `--path`; optional `--full-refresh-keys` |
| `standardize` | `--changed-only` |
| `train` | `--if-changed` skips a known config/corpus snapshot |
| `match` | Required `--mode`; optional `--model-version`; `--new-tf-snapshot` is reserved for `correct` |
| `reconcile` | `--reason` |
| `assemble` | `--touched-only` uses the shared run's touched entity set |

Use `er COMMAND --help` for the full argument list. For detailed diagnostics, see
[profiling](profiling.md).
