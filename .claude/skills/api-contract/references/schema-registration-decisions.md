# Founding JSON-wire registration decisions

This records the backlog named by [#862](https://github.com/athvin/entity-resolution-engine/issues/862),
already registered by [#900](https://github.com/athvin/entity-resolution-engine/pull/900)
for #762. It is a historical decision record, not a live inventory. The catalog,
`er-server/src/openapi.rs` and its bidirectional guards own the current set.

Every entity below was accepted as component vocabulary for an existing JSON
read surface. Its generated `<Name>Wire` schema includes public columns only.
The decision for every row is **register**, with no retained backlog exception.

| Entity | Existing read surface supporting registration |
|---|---|
| `candidate_key` | `read::profile` includes candidate keys measured by a tier-2 profile run. |
| `column_profile` | `read::profile` returns column measurements, with value-bearing fields redacted for viewers. |
| `entity_link` | Entity-link reads in `lib.rs` return links for an entity. |
| `ingest_batch` | Recent and paginated ingest activity in `read.rs`. |
| `matching_model` | The model registry's list/detail surfaces in `lib.rs`. |
| `metric_snapshot` | Recent metrics in `read.rs`. |
| `metric_snapshot_source` | Per-run source metrics in `read.rs`. |
| `pair_label` | Label pagination and review/evaluation reads in `lib.rs`. |
| `profile_finding` | `read::profile` includes findings for the selected run. |
| `profile_run` | `read::profile` exposes run identity and measurement metadata. |
| `record_quality` | Quality distribution and worst-record reads in `read.rs`. |
| `s3_ingest_cursor` | The sources endpoint in `lib.rs` projects cursor rows into source status. |
| `tag` | Active-tag and subject-tag reads through `pg.rs`. |
| `tagging` | Tag-subject and dataset tagging reads through `pg.rs`. |
| `webhook_delivery` | Paginated webhook delivery history in `lib.rs`. |

## A registered entity is not a typed endpoint response

Both OpenAPI audiences include these components. Public audience filtering
selects operations; it does not convert every response into a component
reference. At the #862 reconciliation, responses remain envelope-level and
neither committed document contains a `$ref`. Reference-resolution checks alone
therefore provide no evidence of typed payload coverage. That work belongs to
[#863](https://github.com/athvin/entity-resolution-engine/issues/863) and
[#864](https://github.com/athvin/entity-resolution-engine/issues/864).

Before referencing a component from a typed response, compare it with what the
handler actually serializes. In particular:

- Candidate-key and metric reads use query projections (`ForRunRow`,
  `RecentRow`, `ByRunRow`), which need not match the entity wire shape.
- Profile responses assemble run fields and apply `VALUE_BEARING_COLUMN_FIELDS`
  redaction to column measurements. Quality reads also include aggregate data.
- `model_summary` omits `model_json` and derives comparator metadata; model
  detail handlers add fields to that summary.
- The sources endpoint constructs a source-status object from cursor rows.

Use a response type matching the actual projection and authorization behavior.
Do not substitute an entity reference simply because its component exists.

## Verification

`make check` runs both OpenAPI snapshot tests and the catalog guards:
`every_json_wire_entity_has_a_schema_or_a_declared_gap` rejects missing
registrations and stale exceptions;
`no_schema_is_registered_for_an_entity_that_is_not_on_the_json_wire` rejects
registrations outside the declared JSON wire. Snapshot tests pin both audiences.

For a new registration, regenerate both OpenAPI documents and the internal
TypeScript output using the commands in the parent skill. #862 changed no
registrations: #900 supplied the components and #1064 supplied the public
document, so this reconciliation requires no generated-artifact edits.
