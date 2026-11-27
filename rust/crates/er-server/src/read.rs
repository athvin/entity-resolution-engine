//! The DuckLake-backed read API (`erserver/readapi.py`): the viewer surface that
//! reads an org's resolved results out of its lake. Each request resolves the
//! org to its lake connection from the org's stored `env`, attaches DuckLake
//! read-only, pins a snapshot (DuckLake time-travel), and queries `lake.main.*`.
//!
//! Rows are rendered through DuckDB `to_json` so SQL NULLs and types survive
//! into the response (unlike a plain string projection). Results are a keyset
//! page: `{ items, snapshot, next_cursor }`, the cursor an opaque base64url of
//! `{s: snapshot, k: last_key}` so paging is stable against a moving lake.
//!
//! STATUS: golden-records list landed first (the core resolved-entity read); the
//! other readers (detail, metrics, duplicates, reviews, match-scores,
//! assertions, runs, imports) follow the same attach + envelope.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dedupe_core::Session;
use dedupe_core::session::SessionOptions;
use dedupe_core::session::ducklake::DuckLakeConfig;
use er_config::EntityType;
use serde_json::{Value, json};

/// The golden-records list page size when the caller gives none, and the cap
/// (`readapi.py` `limit <= 200`).
const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;

/// A decoded keyset cursor: the pinned snapshot and the last key emitted.
pub struct Cursor {
    pub snapshot: i64,
    pub key: String,
}

/// Encode an opaque `next_cursor` (base64url of `{"s":snap,"k":key}`).
#[must_use]
pub fn encode_cursor(snapshot: i64, key: &str) -> String {
    let doc = json!({ "s": snapshot, "k": key });
    URL_SAFE_NO_PAD.encode(doc.to_string())
}

/// Decode a cursor; `None` for a malformed one (the handler maps that to 422).
#[must_use]
pub fn decode_cursor(raw: &str) -> Option<Cursor> {
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let doc: Value = serde_json::from_slice(&bytes).ok()?;
    Some(Cursor {
        snapshot: doc.get("s")?.as_i64()?,
        key: doc.get("k")?.as_str()?.to_string(),
    })
}

/// A `%…%` contains-search LIKE pattern: escape the LIKE metacharacters so a user's
/// `%`/`_`/`\` are matched literally, then wrap in wildcards. The result is a bound
/// parameter value (not a SQL literal), paired with `ESCAPE '\'` at the use site.
fn like_contains(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// Open an org's lake read-only from its stored `env`
/// (`readapi.open_lake`): `ER_LAKE_CATALOG` is the post-`ducklake:` metadata
/// (a file catalog locally, `postgres:<dsn>` in production), `ER_LAKE_DATA_PATH`
/// the Parquet location, `ER_LAKE_METADATA_SCHEMA` the lake schema (default
/// `main`), and `ER_LAKE_CATALOG_SCHEMA` DuckLake's `METADATA_SCHEMA` — the
/// per-lake schema of catalog tables inside a shared Postgres metadata
/// database (unset for file catalogs).
///
/// # Errors
/// Returns a message if `env` lacks the catalog or the attach fails (the
/// handler maps this to 503 lake-unavailable).
pub fn open_org_lake(env: &Value) -> Result<Session, String> {
    let catalog = env
        .get("ER_LAKE_CATALOG")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("org env is missing ER_LAKE_CATALOG")?;
    let data_path = env.get("ER_LAKE_DATA_PATH").and_then(Value::as_str);
    let schema = env
        .get("ER_LAKE_METADATA_SCHEMA")
        .and_then(Value::as_str)
        .unwrap_or("main");
    let catalog_schema = env
        .get("ER_LAKE_CATALOG_SCHEMA")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let mut session =
        Session::open_in_memory(SessionOptions::deterministic().with_baked_extensions())
            .map_err(|e| crate::errfmt::chain(&e))?;
    session
        .attach_ducklake(&DuckLakeConfig {
            metadata: catalog.to_string(),
            data_path: data_path.map(str::to_string),
            metadata_schema: catalog_schema.map(str::to_string),
            schema: schema.to_string(),
            read_only: true,
            ..DuckLakeConfig::default()
        })
        .map_err(|e| e.to_string())?;
    Ok(session)
}

/// A keyset page of golden records: `(items, next_cursor)`, snapshot-pinned and
/// ordered by `entity_id` (`readapi.golden_list`). `q` is an optional
/// case-insensitive contains-search over name/email; `after` is the previous
/// page's last `entity_id`.
///
/// # Errors
/// Propagates the lake query error.
pub fn golden_arrow_page(
    session: &Session,
    entity_type: EntityType,
    snapshot: i64,
    q: Option<&str>,
    after: Option<&str>,
    limit: i64,
) -> Result<GoldenArrowPage, String> {
    golden_arrow_page_with_executor(session.connection(), entity_type, snapshot, q, after, limit)
}

/// The same response shaping for embedded and operator-selected remote reads.
pub fn golden_arrow_page_with_executor<E: er_entities::execution::Executor>(
    executor: &E,
    entity_type: EntityType,
    snapshot: i64,
    q: Option<&str>,
    after: Option<&str>,
    limit: i64,
) -> Result<GoldenArrowPage, String> {
    use arrow::array::StringArray;
    use arrow::record_batch::RecordBatch;

    let limit = limit.clamp(1, MAX_LIMIT);
    // The read statements bind `$q` as a LIKE pattern (`... ILIKE $q ESCAPE '\'`),
    // so the contains-search term is wrapped in `%…%` with its LIKE metacharacters
    // escaped here; an empty term means "no filter" (NULL).
    let q_pattern = q.filter(|s| !s.is_empty()).map(like_contains);
    let q = q_pattern.as_deref();

    // Fetch one extra row to decide whether a next page exists, via the generated
    // typed query function (person or account).
    let batches = match entity_type {
        EntityType::Person => er_entities::golden_record::read::list_page_with_executor(
            executor,
            snapshot,
            after,
            q,
            limit + 1,
        ),
        EntityType::Account => er_entities::acct_golden_record::read::list_page_with_executor(
            executor,
            snapshot,
            after,
            q,
            limit + 1,
        ),
    }
    .map_err(|e| e.to_string())?;

    // The read API's contract: every batch is the declared shape. DuckDB's
    // query_arrow carries no entity metadata and its own nullability, so the check
    // is field name + type (not full Schema equality).
    let expected = match entity_type {
        EntityType::Person => er_entities::golden_record::GoldenRecord::arrow_schema(),
        EntityType::Account => er_entities::acct_golden_record::AcctGoldenRecord::arrow_schema(),
    };
    for batch in &batches {
        assert_golden_fields(batch, &expected)?;
    }

    // Cap to `limit` rows (slicing the crossing batch) and derive the next cursor
    // from the last kept row's entity_id (column 0 in both projections).
    let total: i64 = batches.iter().map(|b| b.num_rows() as i64).sum();
    let has_more = total > limit;
    let mut kept: Vec<RecordBatch> = Vec::new();
    let mut remaining = limit;
    let mut last_entity_id: Option<String> = None;
    for batch in batches {
        if remaining <= 0 {
            break;
        }
        let take = i64::min(batch.num_rows() as i64, remaining) as usize;
        let slice = if take == batch.num_rows() {
            batch
        } else {
            batch.slice(0, take)
        };
        if take > 0 {
            let ids = slice
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or("entity_id column is not Utf8")?;
            last_entity_id = Some(ids.value(take - 1).to_string());
        }
        remaining -= take as i64;
        kept.push(slice);
    }
    let next_cursor = if has_more {
        last_entity_id.map(|id| encode_cursor(snapshot, &id))
    } else {
        None
    };
    // Stream with the batches' own (DuckDB) schema so the writer and batches match
    // exactly; an empty page carries the declared schema so the reader still sees
    // the column set.
    let schema = kept
        .first()
        .map_or_else(|| std::sync::Arc::new(expected), RecordBatch::schema);
    Ok(GoldenArrowPage {
        schema,
        batches: kept,
        next_cursor,
    })
}

/// A page of golden records as Arrow batches, capped to the page limit, with the
/// keyset cursor for the next page and the schema to stream.
pub struct GoldenArrowPage {
    pub schema: arrow::datatypes::SchemaRef,
    pub batches: Vec<arrow::record_batch::RecordBatch>,
    pub next_cursor: Option<String>,
}

/// Assert a batch's columns match the entity's declared columns by name and type,
/// ignoring metadata and nullability (which DuckDB's query_arrow does not carry).
fn assert_golden_fields(
    batch: &arrow::record_batch::RecordBatch,
    expected: &arrow::datatypes::Schema,
) -> Result<(), String> {
    let actual = batch.schema();
    if actual.fields().len() != expected.fields().len() {
        return Err(format!(
            "golden read: {} columns, expected {}",
            actual.fields().len(),
            expected.fields().len()
        ));
    }
    for (got, want) in actual.fields().iter().zip(expected.fields().iter()) {
        if got.name() != want.name() || got.data_type() != want.data_type() {
            return Err(format!(
                "golden read: column {}:{:?} != expected {}:{:?}",
                got.name(),
                got.data_type(),
                want.name(),
                want.data_type()
            ));
        }
    }
    Ok(())
}

/// Why resolving a page's start failed: a malformed cursor (the handler maps to
/// 422) vs. a lake snapshot-read failure (503).
pub enum PageStartError {
    BadCursor,
    Lake(String),
}

/// Why an explicitly requested `snapshot` cannot be read (#1229).
#[derive(Debug, PartialEq)]
pub enum PinError {
    /// A cursor pins its own snapshot; asking for a different one at the same
    /// time is a contradiction, not a preference.
    CursorDisagrees { cursor: i64, requested: i64 },
    /// No such snapshot yet.
    NotYet { requested: i64, latest: i64 },
    /// Expired by snapshot retention: the data is gone, and serving a newer
    /// snapshot under the requested number would be a silent substitution.
    Expired { requested: i64, oldest: i64 },
    /// The catalog could not be read.
    Lake(String),
}

/// Apply a request's `snapshot` to a resolved page start.
///
/// `start` is what [`page_start`] resolved: the cursor's snapshot and key, or
/// the latest snapshot with no key. With no `requested`, it passes through
/// unchanged. `bounds` is consulted only for a cursor-less request, and
/// returns `(oldest, latest)` retained.
///
/// # Errors
/// A [`PinError`] naming why the requested snapshot cannot be read.
pub fn pin_requested(
    start: (i64, Option<String>),
    requested: Option<i64>,
    bounds: impl FnOnce() -> Result<(i64, i64), String>,
) -> Result<(i64, Option<String>), PinError> {
    let Some(requested) = requested else {
        return Ok(start);
    };
    if start.1.is_some() {
        return if start.0 == requested {
            Ok(start)
        } else {
            Err(PinError::CursorDisagrees {
                cursor: start.0,
                requested,
            })
        };
    }
    let (oldest, latest) = bounds().map_err(PinError::Lake)?;
    if requested > latest {
        Err(PinError::NotYet { requested, latest })
    } else if requested < oldest {
        Err(PinError::Expired { requested, oldest })
    } else {
        Ok((requested, None))
    }
}

/// Resolve the snapshot + after-key for a request: an explicit `cursor` pins its
/// snapshot and key; otherwise pin the lake's latest snapshot and start at the
/// beginning.
///
/// # Errors
/// [`PageStartError::BadCursor`] for a malformed cursor; [`PageStartError::Lake`]
/// if the lake's latest snapshot can't be read.
pub fn page_start(
    session: &Session,
    cursor: Option<&str>,
) -> Result<(i64, Option<String>), PageStartError> {
    if let Some(raw) = cursor {
        let c = decode_cursor(raw).ok_or(PageStartError::BadCursor)?;
        Ok((c.snapshot, Some(c.key)))
    } else {
        let snap = session
            .latest_snapshot_version()
            .map_err(|e| PageStartError::Lake(e.to_string()))?;
        Ok((snap, None))
    }
}

/// Parse a `limit` query value, defaulting + capping.
#[must_use]
pub fn parse_limit(raw: Option<&str>) -> i64 {
    raw.and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT)
}

/// The lake's latest snapshot version (the pin for a cursor-less read).
///
/// # Errors
/// Propagates the lake query error.
pub fn latest_snapshot(session: &Session) -> Result<i64, String> {
    session.latest_snapshot_version().map_err(|e| e.to_string())
}

/// Serialize generated statement rows to JSON values (the row structs' serde
/// derives carry the DuckDB-compatible rendering, so this replaces the old
/// `to_json` hop byte-for-byte).
fn rows_to_values<T: serde::Serialize>(rows: &[T]) -> Vec<Value> {
    rows.iter()
        .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
        .collect()
}

/// The shipped `confidence` beside every item's `match_probability`: the
/// display integer of the one projection (`dedupe_core::confidence`), so no
/// client ever maps a score itself. Items with no score (a structural
/// escalation is not a scored pair) get no field, matching how their
/// `match_probability` reads null.
///
/// No handler calls it since #1234: every scored item is typed. It stays as the
/// oracle the typed-item tests compare against, which is why it is test-only.
#[cfg(test)]
fn with_confidence(items: &mut [Value]) {
    for item in items {
        let Some(p) = item.get("match_probability").and_then(Value::as_f64) else {
            continue;
        };
        if let Some(obj) = item.as_object_mut() {
            obj.insert(
                "confidence".to_string(),
                Value::from(dedupe_core::confidence::Confidence::from_probability(p).display()),
            );
        }
    }
}

/// The shipped display confidence of one match probability.
fn confidence_of(p: f64) -> i64 {
    dedupe_core::confidence::Confidence::from_probability(p).display()
}

/// Keep the first `limit` rows and derive the next keyset cursor from the last
/// kept row's key when a row crossed the limit: [`typed_page`] without the hop
/// through `Value`, for readers that return a typed item.
fn keyset_page<T>(
    mut rows: Vec<T>,
    snapshot: i64,
    limit: i64,
    key: impl Fn(&T) -> &str,
) -> (Vec<T>, Option<String>) {
    let has_more = rows.len() as i64 > limit;
    rows.truncate(usize::try_from(limit).unwrap_or(0));
    let next = has_more
        .then(|| rows.last().map(|r| encode_cursor(snapshot, key(r))))
        .flatten();
    (rows, next)
}

/// One review-queue item: the stored row plus the shipped `confidence`.
///
/// Spelled out rather than flattened from the generated `PageRow`/`ByIdRow`:
/// those names repeat in every entity's `read` module, so registering them
/// would collide, and the `From` impls destructure each row exhaustively, so a
/// column added to the statement fails to compile here instead of silently
/// leaving the wire.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct ReviewItem {
    pub id: String,
    /// `pair` for a record-pair review, `entity` for an entity-subject one.
    pub subject_type: String,
    pub rec_a_key: Option<String>,
    pub rec_b_key: Option<String>,
    pub entity_id: Option<String>,
    pub reason: String,
    /// Null for a structural escalation, which is not a scored pair.
    pub match_probability: Option<f64>,
    pub waterfall: Option<Value>,
    pub status: String,
    pub first_seen_run_id: String,
    pub last_seen_run_id: String,
    pub resolved_by: Option<String>,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp_opt")]
    #[schema(value_type = Option<String>)]
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The display integer of `match_probability`; absent when it is null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<i64>,
}

macro_rules! review_item_from {
    ($row:path) => {
        impl From<$row> for ReviewItem {
            fn from(row: $row) -> Self {
                let $row {
                    id,
                    subject_type,
                    rec_a_key,
                    rec_b_key,
                    entity_id,
                    reason,
                    match_probability,
                    waterfall,
                    status,
                    first_seen_run_id,
                    last_seen_run_id,
                    resolved_by,
                    resolved_at,
                } = row;
                Self {
                    confidence: match_probability.map(confidence_of),
                    id,
                    subject_type,
                    rec_a_key,
                    rec_b_key,
                    entity_id,
                    reason,
                    match_probability,
                    waterfall,
                    status,
                    first_seen_run_id,
                    last_seen_run_id,
                    resolved_by,
                    resolved_at,
                }
            }
        }
    };
}
review_item_from!(er_entities::review::read::PageRow);
review_item_from!(er_entities::review::read::ByIdRow);

/// One pair the labelling session should look at next: the active score plus
/// the shipped `confidence` (always present — every uncertain pair is scored).
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct UncertainPair {
    pub rec_a_key: String,
    pub rec_b_key: String,
    pub match_probability: f64,
    pub model_version: String,
    /// The comparison evidence blob (`gamma_*`, `mw_*`, `match_weight`).
    pub evidence: Value,
    pub run_id: String,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp")]
    #[schema(value_type = String)]
    pub scored_at: chrono::DateTime<chrono::Utc>,
    pub confidence: i64,
}

impl From<er_entities::match_score::read::UncertainPageRow> for UncertainPair {
    fn from(row: er_entities::match_score::read::UncertainPageRow) -> Self {
        let er_entities::match_score::read::UncertainPageRow {
            rec_a_key,
            rec_b_key,
            match_probability,
            model_version,
            evidence,
            run_id,
            scored_at,
        } = row;
        Self {
            confidence: confidence_of(match_probability),
            rec_a_key,
            rec_b_key,
            match_probability,
            model_version,
            evidence,
            run_id,
            scored_at,
        }
    }
}

/// One active pairwise score (`GET …/match-scores`): the pair, its probability
/// and evidence, and the shipped `confidence` (#1234). The same columns as an
/// [`UncertainPair`], read by a different statement.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct ScoredPair {
    pub rec_a_key: String,
    pub rec_b_key: String,
    pub match_probability: f64,
    pub model_version: String,
    /// The comparison evidence blob (`gamma_*`, `mw_*`, `match_weight`).
    pub evidence: Value,
    pub run_id: String,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp")]
    #[schema(value_type = String)]
    pub scored_at: chrono::DateTime<chrono::Utc>,
    pub confidence: i64,
}

impl From<er_entities::match_score::read::ActivePageRow> for ScoredPair {
    fn from(row: er_entities::match_score::read::ActivePageRow) -> Self {
        let er_entities::match_score::read::ActivePageRow {
            rec_a_key,
            rec_b_key,
            match_probability,
            model_version,
            evidence,
            run_id,
            scored_at,
        } = row;
        Self {
            confidence: confidence_of(match_probability),
            rec_a_key,
            rec_b_key,
            match_probability,
            model_version,
            evidence,
            run_id,
            scored_at,
        }
    }
}

/// One entity's merge plan (`GET …/merge-plans`): its elected master, the
/// golden preview, every member as `source_system:source_record_id`, and the
/// same group confidence the duplicates view shows.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct MergePlan {
    pub entity_id: String,
    pub master_key: Option<String>,
    pub member_count: Option<i64>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub email: Option<String>,
    pub phone_e164: Option<String>,
    pub member_records: Vec<String>,
    /// Null when the group has no within-group scored edge.
    pub group_confidence: Option<i64>,
    pub master_record_key: Option<String>,
    pub member_confidences: std::collections::BTreeMap<String, i64>,
}

/// One group's score from its members and within-group edges.
fn group_score_of(
    members: &[String],
    edges: &[GroupEdge],
) -> Option<dedupe_core::confidence::GroupScore> {
    let triples: Vec<(String, String, f64)> = edges
        .iter()
        .map(|e| (e.rec_a.clone(), e.rec_b.clone(), e.probability))
        .collect();
    dedupe_core::confidence::group_score(members, &triples)
}

/// One within-group evidence edge with its provenance. A scored edge carries
/// the model version and the comparison columns its evidence names — the
/// matched-by and matched-on filters' raw material; an asserted edge is a
/// steward's `always`. A group is `verified` when it carries an asserted edge
/// or one of its member rows carries an active steward verification (2.9).
pub struct GroupEdge {
    pub rec_a: String,
    pub rec_b: String,
    pub probability: f64,
    pub model_version: Option<String>,
    pub fields: Vec<String>,
    pub asserted: bool,
}

/// Within-group evidence for a page of entities, bucketed per entity id so a
/// pair's score never leaks across groups.
type GroupEdges = std::collections::BTreeMap<String, Vec<GroupEdge>>;

/// The comparison columns an evidence blob names: the `gamma_*` / `mw_*` keys
/// with prefixes stripped (`mw_tf_adj_` first — its prefix collides with
/// `mw_`). The blob's `match_weight` total names no column and drops out.
fn evidence_fields(evidence: &Value) -> Vec<String> {
    let Some(map) = evidence.as_object() else {
        return Vec::new();
    };
    map.keys()
        .filter_map(|k| {
            k.strip_prefix("mw_tf_adj_")
                .or_else(|| k.strip_prefix("gamma_"))
                .or_else(|| k.strip_prefix("mw_"))
                .map(str::to_string)
        })
        .collect()
}

/// The within-group edges for an entity-id range, bucketed per entity so a
/// pair's score never leaks across groups.
fn edges_by_entity(
    session: &Session,
    snapshot: i64,
    first: &str,
    last: &str,
) -> Result<GroupEdges, String> {
    let mut by_entity: GroupEdges = std::collections::BTreeMap::new();
    // A lake has `match_scores` only once a match run has written them and
    // `assertions` only once a steward has staged one; either side can
    // legitimately be absent (a freshly resolved lake has no assertions table
    // at all). Each arm is fetched only when its table exists rather than
    // rescued from a planner error, the same reason `merge_plans` probes
    // `record_overlay` before emitting SQL that names it.
    let conn = session.connection();
    if session
        .lake_table_exists("match_scores")
        .map_err(|e| e.to_string())?
    {
        let rows = er_entities::match_score::read::within_groups(conn, snapshot, first, last)
            .map_err(|e| e.to_string())?;
        for row in rows {
            let mut fields = evidence_fields(&row.evidence);
            fields.sort();
            fields.dedup();
            by_entity.entry(row.entity_id).or_default().push(GroupEdge {
                rec_a: row.rec_a_key,
                rec_b: row.rec_b_key,
                probability: row.match_probability,
                model_version: Some(row.model_version),
                fields,
                asserted: false,
            });
        }
    }
    if session
        .lake_table_exists("assertions")
        .map_err(|e| e.to_string())?
    {
        let rows = er_entities::assertion::read::within_groups(conn, snapshot, first, last)
            .map_err(|e| e.to_string())?;
        for row in rows {
            by_entity.entry(row.entity_id).or_default().push(GroupEdge {
                rec_a: row.rec_a_key,
                rec_b: row.rec_b_key,
                probability: row.match_probability,
                model_version: None,
                fields: Vec::new(),
                asserted: true,
            });
        }
    }
    Ok(by_entity)
}

/// Move every field of a generated lake row into a spelled-out response item.
/// The row is destructured exhaustively, so a column added to the statement
/// fails to compile here instead of silently leaving the wire.
macro_rules! item_from_row {
    ($item:ty, $row:path, { $($field:ident),* $(,)? }) => {
        impl From<$row> for $item {
            fn from(row: $row) -> Self {
                let $row { $($field),* } = row;
                Self { $($field),* }
            }
        }
    };
}

/// A person entity's golden record, as of the pinned snapshot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct PersonGolden {
    pub id: String,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub email: Option<String>,
    pub phone_e164: Option<String>,
    pub addr_number: Option<String>,
    pub addr_street: Option<String>,
    pub addr_unit: Option<String>,
    pub addr_city: Option<String>,
    pub addr_region: Option<String>,
    pub addr_postal: Option<String>,
    pub birth_date: Option<chrono::NaiveDate>,
    pub metadata: Option<Value>,
    pub survivorship_version: String,
}
item_from_row!(PersonGolden, er_entities::golden_record::read::ByIdRow, {
    id, given_name, family_name, email, phone_e164, addr_number, addr_street,
    addr_unit, addr_city, addr_region, addr_postal, birth_date, metadata,
    survivorship_version,
});

/// An account entity's golden record, as of the pinned snapshot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct AccountGolden {
    pub id: String,
    pub company_name: Option<String>,
    pub domain: Option<String>,
    pub phone_e164: Option<String>,
    pub addr_street: Option<String>,
    pub addr_city: Option<String>,
    pub addr_region: Option<String>,
    pub addr_postal: Option<String>,
    pub metadata: Option<Value>,
    pub survivorship_version: String,
}
item_from_row!(AccountGolden, er_entities::acct_golden_record::read::ByIdRow, {
    id, company_name, domain, phone_e164, addr_street, addr_city, addr_region,
    addr_postal, metadata, survivorship_version,
});

/// The golden record of whichever entity type the dataset resolves.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum GoldenRow {
    Person(PersonGolden),
    Account(AccountGolden),
}

/// Which member supplied one golden attribute, and by which survivorship rule.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct LineageItem {
    pub attribute: String,
    pub record_key: String,
    pub source_system: String,
    pub source_record_id: String,
    pub rule: String,
}
item_from_row!(LineageItem, er_entities::golden_lineage::read::ByEntityRow, {
    attribute, record_key, source_system, source_record_id, rule,
});
item_from_row!(LineageItem, er_entities::acct_golden_lineage::read::ByEntityRow, {
    attribute, record_key, source_system, source_record_id, rule,
});

/// One member record of an entity, with its active steward flags.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct EntityMember {
    pub source_system: String,
    pub source_record_id: String,
    pub record_key: String,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp")]
    #[schema(value_type = String)]
    pub assigned_at: chrono::DateTime<chrono::Utc>,
    pub run_id: String,
    /// An active `protect` overlay: the record is master for its entity and
    /// cannot be deleted or unmatch-peeled until unprotected.
    pub protected: bool,
    /// An active whole-row steward verification.
    pub verified: bool,
}

/// One event in an entity's history.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct EntityEventItem {
    pub id: String,
    pub seq: i64,
    pub run_id: String,
    pub event_type: String,
    pub details: Value,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp")]
    #[schema(value_type = String)]
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}
item_from_row!(EntityEventItem, er_entities::entity_event::read::ByEntityRow, {
    id, seq, run_id, event_type, details, occurred_at,
});

/// What [`entity_detail`] read, before it is shaped into a [`GoldenDetail`].
pub struct EntityDetailRows {
    pub(crate) golden: GoldenRow,
    pub(crate) members: Vec<EntityMember>,
    pub(crate) lineage: Vec<LineageItem>,
    pub(crate) events: Vec<EntityEventItem>,
}

/// `GET …/golden-records/{entity_id}`: one resolved entity, snapshot-pinned.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct GoldenDetail {
    pub golden: GoldenRow,
    pub members: Vec<EntityMember>,
    pub lineage: Vec<LineageItem>,
    pub events: Vec<EntityEventItem>,
}

impl From<EntityDetailRows> for GoldenDetail {
    fn from(rows: EntityDetailRows) -> Self {
        let EntityDetailRows {
            golden,
            members,
            lineage,
            events,
        } = rows;
        Self {
            golden,
            members,
            lineage,
            events,
        }
    }
}

/// One resolved entity with its members, lineage, and events
/// (`readapi.entity_detail`), snapshot-pinned. `None` if the entity is absent
/// at that snapshot (→ 404).
///
/// # Errors
/// Propagates a lake query error.
pub fn entity_detail(
    session: &Session,
    entity_type: EntityType,
    snapshot: i64,
    entity_id: &str,
) -> Result<Option<EntityDetailRows>, String> {
    let conn = session.connection();
    let err = |e: duckdb::Error| e.to_string();
    let golden = match entity_type {
        EntityType::Person => er_entities::golden_record::read::by_id(conn, snapshot, entity_id)
            .map_err(err)?
            .into_iter()
            .next()
            .map(|row| GoldenRow::Person(row.into())),
        EntityType::Account => {
            er_entities::acct_golden_record::read::by_id(conn, snapshot, entity_id)
                .map_err(err)?
                .into_iter()
                .next()
                .map(|row| GoldenRow::Account(row.into()))
        }
    };
    let Some(golden) = golden else {
        return Ok(None);
    };
    let members =
        er_entities::entity_membership::read::by_entity(conn, snapshot, entity_id).map_err(err)?;
    let lineage: Vec<LineageItem> = match entity_type {
        EntityType::Person => {
            er_entities::golden_lineage::read::by_entity(conn, snapshot, entity_id)
                .map_err(err)?
                .into_iter()
                .map(LineageItem::from)
                .collect()
        }
        EntityType::Account => {
            er_entities::acct_golden_lineage::read::by_entity(conn, snapshot, entity_id)
                .map_err(err)?
                .into_iter()
                .map(LineageItem::from)
                .collect()
        }
    };
    let events = er_entities::entity_event::read::by_entity(conn, snapshot, entity_id)
        .map_err(err)?
        .into_iter()
        .map(EntityEventItem::from)
        .collect();
    // Per-member steward flags: the entity view's action affordances read
    // these (a protected member cannot be deleted or unmatch-peeled without
    // an explicit unprotect first; a verified member shows the mark).
    let protected: std::collections::BTreeSet<String> =
        active_overlay_keys(session, snapshot, "protect")?
            .into_iter()
            .collect();
    let row_verified: std::collections::BTreeSet<String> = active_verifications(session, snapshot)?
        .into_iter()
        .filter(|(_, attribute)| attribute.is_none())
        .map(|(record_key, _)| record_key)
        .collect();
    let members = members
        .into_iter()
        .map(|m| {
            let er_entities::entity_membership::read::ByEntityRow {
                source_system,
                source_record_id,
                record_key,
                assigned_at,
                run_id,
            } = m;
            EntityMember {
                protected: protected.contains(&record_key),
                verified: row_verified.contains(&record_key),
                source_system,
                source_record_id,
                record_key,
                assigned_at,
                run_id,
            }
        })
        .collect();
    Ok(Some(EntityDetailRows {
        golden,
        members,
        lineage,
        events,
    }))
}

/// The member record_keys of one entity at `snapshot`, ascending — empty when
/// the entity is absent or `entity_membership` has not been materialized. The
/// unmerge precheck uses this to know which records it may split and which to
/// keep.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn entity_member_keys(
    session: &Session,
    snapshot: i64,
    entity_id: &str,
) -> Result<Vec<String>, String> {
    match er_entities::entity_membership::read::member_keys(
        session.connection(),
        snapshot,
        entity_id,
    ) {
        Ok(rows) => Ok(rows.into_iter().map(|r| r.record_key).collect()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "entity_membership") {
                Ok(Vec::new())
            } else {
                Err(s)
            }
        }
    }
}

/// The active assertions at `snapshot` as `(assertion_id, rec_a_key, rec_b_key,
/// kind)`, empty when the ledger table is absent. Feeds the unmerge
/// contradiction precheck (combined with the proposed `never` pairs).
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn active_assertions(
    session: &Session,
    snapshot: i64,
) -> Result<Vec<(String, String, String, String)>, String> {
    match er_entities::assertion::read::active_pairs(session.connection(), snapshot) {
        Ok(rows) => Ok(rows
            .into_iter()
            .map(|r| (r.id, r.rec_a_key, r.rec_b_key, r.kind))
            .collect()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "assertions") {
                Ok(Vec::new())
            } else {
                Err(s)
            }
        }
    }
}

/// The active overlay record keys of one kind (`protect` / `suppress` /
/// `delete`) at `snapshot` — empty when the overlay table is absent. The
/// staging prechecks read these: delete refuses a protected record, and a
/// mass merge refuses a protected record as a non-master member.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn active_overlay_keys(
    session: &Session,
    snapshot: i64,
    kind: &str,
) -> Result<Vec<String>, String> {
    match er_entities::record_overlay::read::active_keys_by_kind(
        session.connection(),
        snapshot,
        kind,
    ) {
        Ok(rows) => Ok(rows.into_iter().map(|r| r.record_key).collect()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "record_overlays") {
                Ok(Vec::new())
            } else {
                Err(s)
            }
        }
    }
}

/// The active verification flags (spec 2.9) at `snapshot` as
/// `(record_key, attribute)` pairs — empty when the table is absent (no
/// steward has verified anything yet). A NULL `attribute` marks the whole row.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn active_verifications(
    session: &Session,
    snapshot: i64,
) -> Result<Vec<(String, Option<String>)>, String> {
    match er_entities::record_verification::read::active(session.connection(), snapshot) {
        Ok(rows) => Ok(rows
            .into_iter()
            .map(|r| (r.record_key, r.attribute))
            .collect()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "record_verifications") {
                Ok(Vec::new())
            } else {
                Err(s)
            }
        }
    }
}

/// Server-side filters over the duplicate-group list (`readapi.duplicate_groups`).
///
/// Every dimension is applied while paging: a filter that ran only over the
/// loaded page would report a false empty list for matches sitting on pages
/// never fetched. Size and the single-entity pin push down into the page
/// statement; the rest are decided per group after assembly, because the
/// confidence score and the evidence metadata are computed in Rust and a SQL
/// copy of either would drift.
#[derive(Default)]
pub struct DuplicateFilters {
    /// Group size floor (the page statement floors at 2 regardless).
    pub min_members: Option<i64>,
    /// Group size ceiling — the size filter's "pairs" bucket.
    pub max_members: Option<i64>,
    /// Pin the list to one entity: the member-record search resolves the key
    /// to its entity id and answers through this same assembly.
    pub only_entity: Option<String>,
    /// Group confidence floor and ceiling on the 0–100 display scale. A group
    /// with no evidence (null confidence) matches neither bound.
    pub min_confidence: Option<i64>,
    pub max_confidence: Option<i64>,
    /// Keep groups carrying at least one scored edge from this model version.
    pub model_version: Option<String>,
    /// Keep groups whose scored evidence names this comparison column.
    pub matched_on: Option<String>,
    /// `true` keeps groups carrying an active always-assertion or a row-level
    /// steward verification among their members, `false` keeps groups with
    /// neither.
    pub verified: Option<bool>,
    /// Entity-id sets from the Postgres tag prequery: membership required /
    /// membership forbidden. `None` is no tag filter on that side; an EMPTY
    /// set is a tag that resolved to nothing, which filters accordingly.
    pub tagged: Option<std::collections::HashSet<String>>,
    pub excluded: Option<std::collections::HashSet<String>>,
}

/// One assembled group before JSON rendering — the filters read this shape.
struct GroupItem {
    entity_id: String,
    member_count: i64,
    member_keys: Vec<String>,
    score: Option<dedupe_core::confidence::GroupScore>,
    model_versions: Vec<String>,
    matched_fields: Vec<String>,
    verified: bool,
    preview: Vec<Option<String>>,
}

/// One duplicate group in a **person** dataset: an entity with two or more
/// member records, its evidence metadata, and a golden preview (#1234).
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct PersonDuplicateGroup {
    /// The resolved entity.
    pub entity_id: String,
    /// How many source records resolved to it.
    pub member_count: i64,
    /// The member records, as `<source_system>:<source_record_id>`, ordered.
    pub members: Vec<String>,
    /// The group's confidence (0–100), or null when no member edge is scored.
    pub group_confidence: Option<i64>,
    /// The strongest-attached member, or null when the group is unscored.
    pub master_record_key: Option<String>,
    /// Each scored member's attachment confidence (0–100), by member key.
    pub member_confidences: std::collections::BTreeMap<String, i64>,
    /// The model versions whose scores link the group, deduplicated.
    pub model_versions: Vec<String>,
    /// The comparison fields any within-group edge matched on.
    pub matched_fields: Vec<String>,
    /// Whether a steward asserted or verified the group.
    pub verified: bool,
    /// The golden record's given name, when assembled.
    pub given_name: Option<String>,
    /// The golden record's family name, when assembled.
    pub family_name: Option<String>,
    /// The golden record's email, when assembled.
    pub email: Option<String>,
}

/// One duplicate group in an **account** dataset; the preview is the golden
/// company name and domain rather than a person's name and email (#1234).
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct AccountDuplicateGroup {
    /// The resolved entity.
    pub entity_id: String,
    /// How many source records resolved to it.
    pub member_count: i64,
    /// The member records, as `<source_system>:<source_record_id>`, ordered.
    pub members: Vec<String>,
    /// The group's confidence (0–100), or null when no member edge is scored.
    pub group_confidence: Option<i64>,
    /// The strongest-attached member, or null when the group is unscored.
    pub master_record_key: Option<String>,
    /// Each scored member's attachment confidence (0–100), by member key.
    pub member_confidences: std::collections::BTreeMap<String, i64>,
    /// The model versions whose scores link the group, deduplicated.
    pub model_versions: Vec<String>,
    /// The comparison fields any within-group edge matched on.
    pub matched_fields: Vec<String>,
    /// Whether a steward asserted or verified the group.
    pub verified: bool,
    /// The golden record's company name, when assembled.
    pub company_name: Option<String>,
    /// The golden record's domain, when assembled.
    pub domain: Option<String>,
}

/// A page of duplicate groups, of the dataset's one entity type.
#[derive(Debug, Clone, PartialEq)]
pub enum DuplicateGroups {
    Person(Vec<PersonDuplicateGroup>),
    Account(Vec<AccountDuplicateGroup>),
}

impl DuplicateGroups {
    fn new(entity_type: EntityType) -> Self {
        match entity_type {
            EntityType::Person => Self::Person(Vec::new()),
            EntityType::Account => Self::Account(Vec::new()),
        }
    }

    /// How many groups the page holds.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Person(v) => v.len(),
            Self::Account(v) => v.len(),
        }
    }

    /// Whether the page holds no group.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The groups as the JSON the endpoint serves.
    #[must_use]
    pub fn to_values(&self) -> Vec<Value> {
        match self {
            Self::Person(v) => v.iter().map(|g| json!(g)).collect(),
            Self::Account(v) => v.iter().map(|g| json!(g)).collect(),
        }
    }

    fn push(&mut self, group: GroupItem) {
        let (confidence, master, members) = match group.score {
            Some(score) => (
                Some(score.group.display()),
                Some(score.master),
                score
                    .members
                    .into_iter()
                    .map(|(k, c)| (k, c.display()))
                    .collect(),
            ),
            None => (None, None, std::collections::BTreeMap::new()),
        };
        let mut preview = group.preview.into_iter();
        match self {
            Self::Person(v) => v.push(PersonDuplicateGroup {
                entity_id: group.entity_id,
                member_count: group.member_count,
                members: group.member_keys,
                group_confidence: confidence,
                master_record_key: master,
                member_confidences: members,
                model_versions: group.model_versions,
                matched_fields: group.matched_fields,
                verified: group.verified,
                given_name: preview.next().flatten(),
                family_name: preview.next().flatten(),
                email: preview.next().flatten(),
            }),
            Self::Account(v) => v.push(AccountDuplicateGroup {
                entity_id: group.entity_id,
                member_count: group.member_count,
                members: group.member_keys,
                group_confidence: confidence,
                master_record_key: master,
                member_confidences: members,
                model_versions: group.model_versions,
                matched_fields: group.matched_fields,
                verified: group.verified,
                company_name: preview.next().flatten(),
                domain: preview.next().flatten(),
            }),
        }
    }
}

/// Whether one assembled group passes every active filter dimension.
fn keep_group(filters: &DuplicateFilters, group: &GroupItem) -> bool {
    let confidence = group.score.as_ref().map(|s| s.group.display());
    filters
        .min_confidence
        .is_none_or(|min| confidence.is_some_and(|c| c >= min))
        && filters
            .max_confidence
            .is_none_or(|max| confidence.is_some_and(|c| c <= max))
        && filters
            .model_version
            .as_ref()
            .is_none_or(|model| group.model_versions.contains(model))
        && filters
            .matched_on
            .as_ref()
            .is_none_or(|field| group.matched_fields.contains(field))
        && filters.verified.is_none_or(|v| group.verified == v)
        && filters
            .tagged
            .as_ref()
            .is_none_or(|tagged| tagged.contains(&group.entity_id))
        && filters
            .excluded
            .as_ref()
            .is_none_or(|excluded| !excluded.contains(&group.entity_id))
}

/// A keyset page of duplicate groups: entities with >=2 members, with their
/// member list, evidence metadata and a golden preview
/// (`readapi.duplicate_groups`). Keyset on `entity_id`.
///
/// With filters active the scan keeps going past the first page-sized chunk
/// until the page fills or the groups run out, bounded by a scan cap; the
/// cursor always encodes the last SCANNED entity id, so the next request
/// continues the filter rather than restarting it.
///
/// # Errors
/// Propagates the lake query error.
pub fn duplicates(
    session: &Session,
    entity_type: EntityType,
    snapshot: i64,
    after: Option<&str>,
    filters: &DuplicateFilters,
    limit: i64,
) -> Result<(DuplicateGroups, Option<String>), String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let conn = session.connection();
    // Wider than the page: the Rust-side filters drop groups after assembly,
    // and a chunk exactly the page size would leave a filtered page short on
    // every request. The cap bounds a filter that matches almost nothing; the
    // caller continues the scan with the returned cursor.
    const SCAN_CHUNK: i64 = 200;
    const SCAN_CAP: i64 = 10_000;
    let mut items = DuplicateGroups::new(entity_type);
    let mut after = after.map(str::to_string);
    let mut last_scanned: Option<String> = None;
    let mut scanned: i64 = 0;
    let mut more = true;
    while (items.len() as i64) < limit && more && scanned < SCAN_CAP {
        let groups = er_entities::entity_membership::read::duplicate_groups_page(
            conn,
            snapshot,
            after.as_deref(),
            filters.min_members,
            filters.max_members,
            filters.only_entity.as_deref(),
            SCAN_CHUNK,
        )
        .map_err(|e| e.to_string())?;
        if groups.is_empty() {
            more = false;
            break;
        }
        scanned += groups.len() as i64;
        let chunk_len = groups.len();
        let chunk_full = chunk_len as i64 == SCAN_CHUNK;
        let chunk_last = groups.last().map(|g| g.entity_id.clone());
        let mut filled_at: Option<usize> = None;
        for (idx, group) in assemble_group_items(session, entity_type, snapshot, &groups)?
            .into_iter()
            .enumerate()
        {
            if !keep_group(filters, &group) {
                continue;
            }
            let id = group.entity_id.clone();
            items.push(group);
            last_scanned = Some(id);
            if items.len() as i64 == limit {
                filled_at = Some(idx);
                break;
            }
        }
        if let Some(idx) = filled_at {
            // The page filled mid-scan: the cursor is the last EMITTED group,
            // so rows past the fill point — never scanned by the next request
            // otherwise — are reached by it.
            more = idx + 1 < chunk_len || chunk_full;
        } else {
            // The whole chunk was scanned without filling the page: continue
            // from the chunk's end while it came back full.
            last_scanned = chunk_last;
            more = chunk_full;
        }
        // A pinned entity id yields at most one row; never loop for it.
        if filters.only_entity.is_some() {
            more = false;
        }
        after.clone_from(&last_scanned);
    }
    let next_cursor = if more {
        last_scanned.map(|last| encode_cursor(snapshot, &last))
    } else {
        None
    };
    Ok((items, next_cursor))
}

/// Assemble one scanned chunk of group rows into filterable items: the member
/// lists, golden previews and within-group evidence for the chunk's id range,
/// then the per-group confidence score and evidence metadata.
fn assemble_group_items(
    session: &Session,
    entity_type: EntityType,
    snapshot: i64,
    groups: &[er_entities::entity_membership::read::DuplicateGroupsPageRow],
) -> Result<Vec<GroupItem>, String> {
    use std::collections::BTreeMap;

    let Some((first, last)) = groups.first().map(|f| {
        (
            f.entity_id.as_str(),
            groups[groups.len() - 1].entity_id.as_str(),
        )
    }) else {
        return Ok(Vec::new());
    };

    // The chunk's within-group evidence, fetched once for the whole id range
    // rather than per group: active scored edges plus always-assertions.
    let edges = edges_by_entity(session, snapshot, first, last)?;

    // Row-level steward verifications (spec 2.9), one fetch for the chunk:
    // a group counts as verified when a member ROW is verified, alongside the
    // always-assertion signal the filter has always read. Cell-level flags do
    // not mark the group.
    let row_verified: std::collections::BTreeSet<String> = active_verifications(session, snapshot)?
        .into_iter()
        .filter(|(_, attribute)| attribute.is_none())
        .map(|(record_key, _)| record_key)
        .collect();

    // The members of every entity in the chunk's id range, already ordered by
    // (entity_id, record_key) — so each group's list is record_key-ordered.
    let conn = session.connection();
    let members =
        er_entities::entity_membership::read::members_in_range(conn, snapshot, first, last)
            .map_err(|e| e.to_string())?;
    let mut member_lists: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for m in &members {
        member_lists
            .entry(m.entity_id.as_str())
            .or_default()
            .push(format!("{}:{}", m.source_system, m.source_record_id));
    }

    // The golden preview per entity in the range (person: name+email; account:
    // company+domain). A group whose golden row is absent previews as nulls,
    // like the old LEFT JOIN.
    let mut previews: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
    match entity_type {
        EntityType::Person => {
            let rows =
                er_entities::golden_record::read::preview_in_range(conn, snapshot, first, last)
                    .map_err(|e| e.to_string())?;
            for r in rows {
                previews.insert(r.id, vec![r.given_name, r.family_name, r.email]);
            }
        }
        EntityType::Account => {
            let rows = er_entities::acct_golden_record::read::preview_in_range(
                conn, snapshot, first, last,
            )
            .map_err(|e| e.to_string())?;
            for r in rows {
                previews.insert(r.id, vec![r.company_name, r.domain]);
            }
        }
    }

    Ok(groups
        .iter()
        .map(|g| {
            let member_keys = member_lists
                .remove(g.entity_id.as_str())
                .unwrap_or_default();
            let group_edges: &[GroupEdge] =
                edges.get(g.entity_id.as_str()).map_or(&[], Vec::as_slice);
            let triples: Vec<(String, String, f64)> = group_edges
                .iter()
                .map(|e| (e.rec_a.clone(), e.rec_b.clone(), e.probability))
                .collect();
            let score = dedupe_core::confidence::group_score(&member_keys, &triples);
            let mut model_versions: Vec<String> = group_edges
                .iter()
                .filter_map(|e| e.model_version.clone())
                .collect();
            model_versions.sort();
            model_versions.dedup();
            let mut matched_fields: Vec<String> = group_edges
                .iter()
                .flat_map(|e| e.fields.iter().cloned())
                .collect();
            matched_fields.sort();
            matched_fields.dedup();
            let preview: Vec<Option<String>> = (0..preview_field_count(entity_type))
                .map(|i| {
                    previews
                        .get(&g.entity_id)
                        .and_then(|vals| vals.get(i).cloned())
                        .flatten()
                })
                .collect();
            let verified = group_edges.iter().any(|e| e.asserted)
                || member_keys.iter().any(|k| row_verified.contains(k));
            GroupItem {
                entity_id: g.entity_id.clone(),
                member_count: g.member_count.unwrap_or(0),
                member_keys,
                score,
                model_versions,
                matched_fields,
                verified,
                preview,
            }
        })
        .collect())
}

/// How many golden preview fields the entity type carries (person 3, account 2).
fn preview_field_count(entity_type: EntityType) -> usize {
    match entity_type {
        EntityType::Person => 3,
        EntityType::Account => 2,
    }
}

/// The composite key separator for `match_scores` cursors (`readapi` joins the
/// pair on `\x1f`).
const PAIR_SEP: char = '\u{1f}';

/// A keyset page of active pairwise match scores (`readapi.match_scores_list`),
/// optionally within a probability band. Keyset on the `(rec_a_key, rec_b_key)`
/// pair, carried in the cursor key as `rec_a_key\x1frec_b_key`.
///
/// # Errors
/// Propagates the lake query error.
pub fn match_scores_list(
    session: &Session,
    snapshot: i64,
    after: Option<&str>,
    min_prob: Option<f64>,
    max_prob: Option<f64>,
    limit: i64,
) -> Result<(Vec<ScoredPair>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    // The composite cursor splits into the statement's expanded tuple predicate
    // ($after_a/$after_b: `a > ka OR (a = ka AND b > kb)`).
    let pair = after.map(|a| a.split_once(PAIR_SEP).unwrap_or((a, "")));
    let rows = er_entities::match_score::read::active_page(
        session.connection(),
        snapshot,
        pair.map(|(ka, _)| ka),
        pair.map(|(_, kb)| kb),
        min_prob,
        max_prob,
        limit + 1,
    )
    .map_err(|e| e.to_string())?;
    let has_more = rows.len() as i64 > limit;
    let page = &rows[..rows.len().min(usize::try_from(limit).unwrap_or(0))];
    let next_cursor = has_more
        .then(|| {
            page.last().map(|r| {
                encode_cursor(
                    snapshot,
                    &format!("{}{PAIR_SEP}{}", r.rec_a_key, r.rec_b_key),
                )
            })
        })
        .flatten();
    let items = page.iter().cloned().map(ScoredPair::from).collect();
    Ok((items, next_cursor))
}

/// Pairs the model is least sure about, nearest the decision threshold first,
/// excluding any whose pair is already labelled.
///
/// Uncertainty sampling is the whole point: pairs the model already scores at
/// 0.99 or 0.01 only confirm what it knows, while the ones either side of the
/// threshold are where its m/u estimates are actually wrong. Labelling in score
/// order instead would spend a human's attention on the easy cases.
///
/// `labelled` is the set of `(rec_a_key, rec_b_key)` already recorded; it is
/// filtered HERE rather than in SQL because labels live in the control plane and
/// scores live in the lake, and there is no join across them. Over-fetching by
/// the number already labelled keeps a full page coming back.
///
/// # Errors
/// Propagates the lake query error.
pub fn uncertain_pairs(
    session: &Session,
    snapshot: i64,
    threshold: f64,
    band: (f64, f64),
    labelled: &std::collections::BTreeSet<(String, String)>,
    limit: i64,
) -> Result<Vec<UncertainPair>, String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let over = limit.saturating_add(i64::try_from(labelled.len()).unwrap_or(0));
    let rows = er_entities::match_score::read::uncertain_page(
        session.connection(),
        snapshot,
        threshold,
        band.0,
        band.1,
        over.clamp(1, MAX_LIMIT * 4),
    )
    .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|r| !labelled.contains(&(r.rec_a_key.clone(), r.rec_b_key.clone())))
        .take(usize::try_from(limit).unwrap_or(0))
        .map(UncertainPair::from)
        .collect())
}

/// One retained metric snapshot, with the rates derived from it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct InsightPoint {
    pub run_id: String,
    /// chrono's display form (`YYYY-MM-DD HH:MM:SS[.f] UTC`).
    pub captured_at: String,
    pub records: i64,
    pub entities: i64,
    pub duplicate_records: i64,
    pub duplicate_groups: i64,
    pub largest_group: i64,
    pub review_backlog: i64,
    pub auto_merge_pairs: i64,
    pub review_band_pairs: i64,
    /// Duplicate records over records; null for an empty corpus.
    pub duplicate_rate: Option<f64>,
    /// Records over entities; null when there are none.
    pub records_per_entity: Option<f64>,
}

/// The retained metric series, newest first, with the derived rates a caller
/// would otherwise compute wrong.
///
/// `duplicate_rate` is duplicate RECORDS over records, not groups over
/// entities: counting groups calls a five-record cluster and a two-record
/// cluster the same amount of duplication, which is how a worsening corpus
/// looks flat. Derived here rather than stored so a definition change does not
/// need a backfill.
///
/// # Errors
/// Propagates the lake query error.
pub fn insights(session: &Session, snapshot: i64, limit: i64) -> Result<Vec<InsightPoint>, String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let rows = er_entities::metric_snapshot::read::recent(session.connection(), snapshot, limit)
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| {
            let rate = |num: i64, den: i64| {
                if den > 0 {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a corpus large enough to lose f64 precision here is far \
                                  past what a rate reported to three decimals can express"
                    )]
                    Some(num as f64 / den as f64)
                } else {
                    // No records is not a 0% duplicate rate — it is no answer.
                    // Reporting 0.0 would draw a reassuring flat line for an
                    // empty corpus.
                    None
                }
            };
            InsightPoint {
                run_id: r.run_id.clone(),
                captured_at: r.captured_at.to_string(),
                records: r.records,
                entities: r.entities,
                duplicate_records: r.duplicate_records,
                duplicate_groups: r.duplicate_groups,
                largest_group: r.largest_group,
                review_backlog: r.review_backlog,
                auto_merge_pairs: r.auto_merge_pairs,
                review_band_pairs: r.review_band_pairs,
                duplicate_rate: rate(r.duplicate_records, r.records),
                records_per_entity: rate(r.records, r.entities),
            }
        })
        .collect())
}

/// The record-quality score distribution, by band.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct QualityBuckets {
    pub below_50: i64,
    pub between_50_80: i64,
    pub at_least_80: i64,
}

/// The mean of each quality dimension; null when nothing is scored.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct QualityDimensions {
    pub completeness: Option<f64>,
    pub validity: Option<f64>,
    pub freshness: Option<f64>,
}

/// One of the lowest-scoring records.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct WorstRecord {
    pub record_key: String,
    pub score: f64,
    pub completeness: f64,
    pub validity: Option<f64>,
    pub freshness: Option<f64>,
}

/// A configured dataset's record-quality report.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct QualityReport {
    /// Always true.
    pub configured: bool,
    pub scored: i64,
    pub mean_score: Option<f64>,
    pub min_score: Option<f64>,
    pub max_score: Option<f64>,
    pub buckets: QualityBuckets,
    pub dimensions: QualityDimensions,
    pub worst: Vec<WorstRecord>,
}

/// A dataset with no quality rules configured: nothing has been scored.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct QualityUnconfigured {
    /// Always 0.
    pub scored: i64,
    /// Always false.
    pub configured: bool,
}

/// `GET …/quality`: the report, or the unconfigured answer.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum Quality {
    Report(QualityReport),
    Unconfigured(QualityUnconfigured),
}

/// The corpus's data-quality distribution and its worst records.
///
/// Buckets as well as a mean, because a mean of 70 can be every record at 70 or
/// half at 95 and half at 45, and those need different work. The per-dimension
/// means say WHICH dimension is dragging, which a single score cannot.
///
/// # Errors
/// Propagates the lake query error. A lake with no `record_qualities` relation —
/// quality scoring unconfigured — reads as unscored rather than failing.
pub fn quality(session: &Session, snapshot: i64, limit: i64) -> Result<Quality, String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let dist = match er_entities::record_quality::read::distribution(session.connection(), snapshot)
    {
        Ok(rows) => rows,
        // An unconfigured tenant has no relation at all, and "nothing has been
        // scored" is the honest answer to the question rather than an error.
        Err(e) if is_missing_relation(&e.to_string()) => {
            return Ok(Quality::Unconfigured(QualityUnconfigured {
                scored: 0,
                configured: false,
            }));
        }
        Err(e) => return Err(e.to_string()),
    };
    let worst =
        er_entities::record_quality::read::worst(session.connection(), snapshot, 100.0, limit)
            .map_err(|e| e.to_string())?;
    let d = dist.first();
    Ok(Quality::Report(QualityReport {
        configured: true,
        scored: d.and_then(|r| r.scored).unwrap_or(0),
        mean_score: d.and_then(|r| r.mean_score),
        min_score: d.and_then(|r| r.min_score),
        max_score: d.and_then(|r| r.max_score),
        buckets: QualityBuckets {
            below_50: d.and_then(|r| r.below_50).unwrap_or(0),
            between_50_80: d.and_then(|r| r.between_50_80).unwrap_or(0),
            at_least_80: d.and_then(|r| r.at_least_80).unwrap_or(0),
        },
        dimensions: QualityDimensions {
            completeness: d.and_then(|r| r.mean_completeness),
            validity: d.and_then(|r| r.mean_validity),
            freshness: d.and_then(|r| r.mean_freshness),
        },
        worst: worst
            .iter()
            .map(|r| WorstRecord {
                record_key: r.record_key.clone(),
                score: r.score,
                completeness: r.completeness,
                validity: r.validity,
                freshness: r.freshness,
            })
            .collect(),
    }))
}

/// Whether a lake error is "that relation does not exist", which for an
/// optional relation means "not configured" rather than "broken".
fn is_missing_relation(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    (m.contains("does not exist") || m.contains("not found") || m.contains("no such"))
        && m.contains("record_qualities")
}

/// Parse an optional `f64` query value (e.g. a probability band bound).
#[must_use]
pub fn parse_f64(raw: Option<&str>) -> Option<f64> {
    raw.and_then(|s| s.parse::<f64>().ok())
}

/// The server-side cap on a `records?key=` lookup (`readapi`: "caps the list at
/// 50").
pub const MAX_KEYS: usize = 50;

/// Collect the repeated `key=` values from a raw query string, percent-decoded,
/// de-duplicated (order-preserving), and capped at [`MAX_KEYS`].
#[must_use]
pub fn keys_from_query(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else { return Vec::new() };
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(raw).unwrap_or_default();
    let mut out = Vec::new();
    for (k, v) in pairs {
        if k == "key" && !v.is_empty() && !out.contains(&v) {
            out.push(v);
            if out.len() >= MAX_KEYS {
                break;
            }
        }
    }
    out
}

/// Standardized source records by `record_key` (`readapi.records_by_key`),
/// snapshot-pinned. Each item is `{record_key, source_system, source_record_id,
/// attributes:{…}}`. Empty `keys` yields no items.
///
/// # Errors
/// Propagates the lake query error.
pub fn records_by_key(
    session: &Session,
    snapshot: i64,
    keys: &[String],
) -> Result<Vec<Value>, String> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    // The lake IN-list idiom: the keys cross as one chr(31)-delimited varchar
    // and split in SQL (duckdb-rs has no LIST binding).
    let joined = keys.join("\u{1f}");
    let rows = er_entities::int_std_record::read::by_keys(session.connection(), snapshot, &joined)
        .map_err(|e| e.to_string())?;
    // A deleted record's cell data is no longer served (spec 2.9): delete is
    // stronger than suppress, which keeps the record readable. Protect wins
    // here exactly as in the reconcile corpus: a protected record is never
    // deleted out from under its pin. Verification flags ride along so the
    // member-values grid can mark verified cells.
    let protected: std::collections::BTreeSet<String> =
        active_overlay_keys(session, snapshot, "protect")?
            .into_iter()
            .collect();
    let deleted: std::collections::BTreeSet<String> =
        active_overlay_keys(session, snapshot, "delete")?
            .into_iter()
            .filter(|k| !protected.contains(k))
            .collect();
    let mut row_verified: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut cell_verified: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for (record_key, attribute) in active_verifications(session, snapshot)? {
        match attribute {
            None => {
                row_verified.insert(record_key);
            }
            Some(attr) => cell_verified.entry(record_key).or_default().push(attr),
        }
    }
    // The old `struct_pack` nesting, rebuilt in Rust: the identity columns on
    // top, every other column under `attributes`. Serializing the typed row
    // first keeps the DuckDB-compatible timestamp rendering.
    Ok(rows
        .iter()
        .filter(|r| !deleted.contains(&r.record_key))
        .map(|r| {
            let Ok(Value::Object(mut all)) = serde_json::to_value(r) else {
                return Value::Null;
            };
            let mut top = serde_json::Map::new();
            for key in ["record_key", "source_system", "source_record_id"] {
                if let Some(v) = all.remove(key) {
                    top.insert(key.to_string(), v);
                }
            }
            top.insert(
                "verified".to_string(),
                Value::from(row_verified.contains(&r.record_key)),
            );
            top.insert(
                "verified_attributes".to_string(),
                Value::Array(
                    cell_verified
                        .get(&r.record_key)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(Value::from)
                        .collect(),
                ),
            );
            top.insert("attributes".to_string(), Value::Object(all));
            Value::Object(top)
        })
        .collect())
}

/// Whether a lake query error is "the table isn't there" — expected for a lake
/// whose review/assertion/run/ingest-tracking stages haven't populated these
/// relations yet, in which case the reader returns an empty page rather than an
/// error.
fn is_missing_table(err: &str, relation: &str) -> bool {
    err.contains(&format!(
        "Catalog Error: Table with name {relation} does not exist"
    ))
}

fn active_protect_overlay(session: &Session) -> Result<bool, String> {
    match er_entities::record_overlay::read::any_active_protect(session.connection()) {
        Ok(rows) => Ok(!rows.is_empty()),
        Err(e) => {
            let error = e.to_string();
            if is_missing_table(&error, "record_overlays") {
                Ok(false)
            } else {
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod typed_item_tests {
    //! #864: each typed item carries exactly what the `rows_to_values` +
    //! `with_confidence` hop it replaced did — equal as parsed values (a
    //! `Value` map sorts its keys; a struct keeps declaration order).
    use super::*;

    fn review(match_probability: Option<f64>) -> er_entities::review::read::PageRow {
        er_entities::review::read::PageRow {
            id: "r1".into(),
            subject_type: "pair".into(),
            rec_a_key: Some("a".into()),
            rec_b_key: Some("b".into()),
            entity_id: None,
            reason: "gray_band".into(),
            match_probability,
            waterfall: Some(json!({"gamma_email": 2})),
            status: "open".into(),
            first_seen_run_id: "run1".into(),
            last_seen_run_id: "run2".into(),
            resolved_by: None,
            resolved_at: Some(
                chrono::DateTime::from_timestamp(1_700_000_000, 120_000_000).unwrap(),
            ),
        }
    }

    #[test]
    fn a_review_item_is_the_row_plus_its_confidence() {
        // Scored, and a structural escalation with no score (no `confidence`).
        for p in [Some(0.876), None] {
            let row = review(p);
            let mut old = rows_to_values(std::slice::from_ref(&row));
            with_confidence(&mut old);
            assert_eq!(old[0], serde_json::to_value(ReviewItem::from(row)).unwrap());
        }
        assert!(
            serde_json::to_value(ReviewItem::from(review(None)))
                .unwrap()
                .get("confidence")
                .is_none()
        );
        let er_entities::review::read::PageRow {
            id,
            subject_type,
            rec_a_key,
            rec_b_key,
            entity_id,
            reason,
            match_probability,
            waterfall,
            status,
            first_seen_run_id,
            last_seen_run_id,
            resolved_by,
            resolved_at,
        } = review(Some(0.5));
        let by_id = er_entities::review::read::ByIdRow {
            id,
            subject_type,
            rec_a_key,
            rec_b_key,
            entity_id,
            reason,
            match_probability,
            waterfall,
            status,
            first_seen_run_id,
            last_seen_run_id,
            resolved_by,
            resolved_at,
        };
        let mut old = rows_to_values(std::slice::from_ref(&by_id));
        with_confidence(&mut old);
        assert_eq!(
            old[0],
            serde_json::to_value(ReviewItem::from(by_id)).unwrap()
        );
    }

    #[test]
    fn an_uncertain_pair_is_the_row_plus_its_confidence() {
        let row = er_entities::match_score::read::UncertainPageRow {
            rec_a_key: "a".into(),
            rec_b_key: "b".into(),
            match_probability: 0.51,
            model_version: "m1".into(),
            evidence: json!({"mw_email": 1.5, "match_weight": 2.0}),
            run_id: "run".into(),
            scored_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        };
        let mut old = rows_to_values(std::slice::from_ref(&row));
        with_confidence(&mut old);
        assert_eq!(
            old[0],
            serde_json::to_value(UncertainPair::from(row)).unwrap()
        );
    }

    #[test]
    fn a_keyset_page_cuts_at_the_limit_and_cursors_on_the_last_kept_row() {
        let rows = vec!["a", "b", "c"];
        let (kept, next) = keyset_page(rows.clone(), 9, 2, |r| r);
        assert_eq!(kept, vec!["a", "b"]);
        assert_eq!(next, Some(encode_cursor(9, "b")));
        let (kept, next) = keyset_page(rows, 9, 3, |r| r);
        assert_eq!(kept.len(), 3);
        assert_eq!(next, None);
    }

    fn plan(scored: bool) -> MergePlan {
        let mut plan = MergePlan {
            entity_id: "e1".into(),
            master_key: None,
            member_count: Some(2),
            given_name: Some("Ann, Jr".into()),
            family_name: None,
            email: Some("a@x".into()),
            phone_e164: None,
            member_records: vec!["crm:1".into(), "erp:2".into()],
            group_confidence: None,
            master_record_key: None,
            member_confidences: std::collections::BTreeMap::new(),
        };
        if scored {
            plan.group_confidence = Some(91);
            plan.master_record_key = Some("crm:1".into());
            plan.member_confidences = [("crm:1".to_string(), 93), ("erp:2".to_string(), 89)].into();
        }
        plan
    }

    #[test]
    fn a_merge_plan_carries_the_keys_the_map_build_did() {
        // The previous build: preview columns, then `insert_group_score`.
        for (scored, group, master, members) in [
            (false, json!(null), json!(null), json!({})),
            (
                true,
                json!(91),
                json!("crm:1"),
                json!({"crm:1": 93, "erp:2": 89}),
            ),
        ] {
            assert_eq!(
                json!({
                    "entity_id": "e1", "master_key": null, "member_count": 2,
                    "given_name": "Ann, Jr", "family_name": null, "email": "a@x",
                    "phone_e164": null, "member_records": ["crm:1", "erp:2"],
                    "group_confidence": group, "master_record_key": master,
                    "member_confidences": members,
                }),
                serde_json::to_value(plan(scored)).unwrap()
            );
        }
    }

    #[test]
    fn the_merge_plan_csv_renders_nulls_empty_and_quotes_commas() {
        assert_eq!(
            merge_plans_csv(&[plan(false), plan(true)]),
            "entity_id,master_key,member_count,member_records,golden_given_name,\
             golden_family_name,golden_email,golden_phone_e164,group_confidence\n\
             e1,,2,crm:1;erp:2,\"Ann, Jr\",,a@x,,\n\
             e1,,2,crm:1;erp:2,\"Ann, Jr\",,a@x,,91\n"
        );
    }
}

#[cfg(test)]
mod typed_detail_tests {
    //! #864 batch 4: each spelled-out item serializes to exactly what
    //! `rows_to_values` made of its generated row (equal as parsed values).
    use super::*;

    fn at() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000, 120_000_000).unwrap()
    }

    fn same<R: serde::Serialize + Clone, T: From<R> + serde::Serialize>(row: R) {
        let old = rows_to_values(std::slice::from_ref(&row)).remove(0);
        assert_eq!(old, serde_json::to_value(T::from(row)).unwrap());
    }

    #[test]
    fn golden_lineage_and_event_items_match_their_rows() {
        same::<_, PersonGolden>(er_entities::golden_record::read::ByIdRow {
            id: "e1".into(),
            given_name: Some("Ann".into()),
            family_name: None,
            email: Some("a@x".into()),
            phone_e164: None,
            addr_number: Some("1".into()),
            addr_street: None,
            addr_unit: None,
            addr_city: Some("Leeds".into()),
            addr_region: None,
            addr_postal: None,
            birth_date: chrono::NaiveDate::from_ymd_opt(1990, 2, 3),
            metadata: Some(json!({"k": 1})),
            survivorship_version: "v1".into(),
        });
        same::<_, AccountGolden>(er_entities::acct_golden_record::read::ByIdRow {
            id: "e2".into(),
            company_name: Some("Acme".into()),
            domain: Some("acme.test".into()),
            phone_e164: None,
            addr_street: None,
            addr_city: None,
            addr_region: None,
            addr_postal: None,
            metadata: None,
            survivorship_version: "v1".into(),
        });
        same::<_, LineageItem>(er_entities::golden_lineage::read::ByEntityRow {
            attribute: "email".into(),
            record_key: "crm:1".into(),
            source_system: "crm".into(),
            source_record_id: "1".into(),
            rule: "most_recent".into(),
        });
        same::<_, LineageItem>(er_entities::acct_golden_lineage::read::ByEntityRow {
            attribute: "domain".into(),
            record_key: "crm:2".into(),
            source_system: "crm".into(),
            source_record_id: "2".into(),
            rule: "source_priority".into(),
        });
        same::<_, EntityEventItem>(er_entities::entity_event::read::ByEntityRow {
            id: "ev".into(),
            seq: 3,
            run_id: "run".into(),
            event_type: "merged".into(),
            details: json!({"from": ["a", "b"]}),
            occurred_at: at(),
        });
    }

    #[test]
    fn run_and_run_event_items_match_their_rows() {
        let run = er_entities::run::read::RecentRow {
            id: "run1".into(),
            mode: "full".into(),
            status: "succeeded".into(),
            started_at: at(),
            ended_at: None,
            config_hash: "h".into(),
            model_version: Some("m1".into()),
            tf_snapshot_id: None,
            rebuild_reason: Some("config".into()),
            snapshot_end: Some(7),
        };
        // The row's `snapshot_end` is served as `snapshot` (#1229), the name
        // the result reads take it back under; every other key is the row's.
        let row_as_served = |run: &er_entities::run::read::RecentRow| {
            let mut old = rows_to_values(std::slice::from_ref(run)).remove(0);
            let snapshot = old.as_object_mut().unwrap().remove("snapshot_end").unwrap();
            old["snapshot"] = snapshot;
            old
        };
        // No threshold: no `auto_merged_pairs` key at all.
        assert_eq!(
            row_as_served(&run),
            serde_json::to_value(RunItem::from(run.clone())).unwrap()
        );
        // With one, the count the old map insert added.
        let mut old = row_as_served(&run);
        old["auto_merged_pairs"] = json!(4_u64);
        let mut item = RunItem::from(run);
        item.auto_merged_pairs = Some(4);
        assert_eq!(old, serde_json::to_value(item).unwrap());
        same::<_, RunEventItem>(er_entities::entity_event::read::ByRunRow {
            id: "ev".into(),
            seq: 9,
            run_id: "run1".into(),
            entity_id: "e1".into(),
            event_type: "created".into(),
            details: None,
            occurred_at: at(),
        });
    }

    #[test]
    fn metrics_quality_and_insight_bodies_keep_their_keys() {
        fn v<T: serde::Serialize>(x: &T) -> Value {
            serde_json::to_value(x).unwrap()
        }
        assert_eq!(
            json!({
                "records": 10, "entities": 7, "duplicate_groups": 2,
                "records_in_duplicate_groups": 5, "open_reviews": 1, "snapshot": 3,
            }),
            v(&Metrics {
                records: 10,
                entities: 7,
                duplicate_groups: 2,
                records_in_duplicate_groups: 5,
                open_reviews: 1,
                snapshot: 3,
            })
        );
        let mut by_attribute = serde_json::Map::new();
        by_attribute.insert("email".into(), json!(0.5));
        assert_eq!(
            json!({
                "dq_score": 0.75,
                "completeness": { "overall": 0.5, "by_attribute": Value::Object(by_attribute) },
                "junk_rate": 0.0, "stale_rate": 0.25, "duplicate_rate": 0.5,
                "records": 10, "entities": 7, "stale_days": 365, "snapshot": 3,
            }),
            v(&DataQuality {
                dq_score: 0.75,
                completeness: Completeness {
                    overall: 0.5,
                    by_attribute: [("email".to_string(), 0.5)].into(),
                },
                junk_rate: 0.0,
                stale_rate: 0.25,
                duplicate_rate: 0.5,
                records: 10,
                entities: 7,
                stale_days: 365,
                snapshot: 3,
            })
        );
        assert_eq!(
            json!({ "scored": 0, "configured": false }),
            v(&Quality::Unconfigured(QualityUnconfigured {
                scored: 0,
                configured: false,
            }))
        );
        assert_eq!(
            json!({
                "configured": true, "scored": 4, "mean_score": 71.5,
                "min_score": 20.0, "max_score": null,
                "buckets": { "below_50": 1, "between_50_80": 2, "at_least_80": 1 },
                "dimensions": { "completeness": 0.9, "validity": null, "freshness": 0.4 },
                "worst": [{
                    "record_key": "crm:1", "score": 20.0, "completeness": 0.2,
                    "validity": null, "freshness": 0.1,
                }],
            }),
            v(&Quality::Report(QualityReport {
                configured: true,
                scored: 4,
                mean_score: Some(71.5),
                min_score: Some(20.0),
                max_score: None,
                buckets: QualityBuckets {
                    below_50: 1,
                    between_50_80: 2,
                    at_least_80: 1,
                },
                dimensions: QualityDimensions {
                    completeness: Some(0.9),
                    validity: None,
                    freshness: Some(0.4),
                },
                worst: vec![WorstRecord {
                    record_key: "crm:1".into(),
                    score: 20.0,
                    completeness: 0.2,
                    validity: None,
                    freshness: Some(0.1),
                }],
            }))
        );
        assert_eq!(
            json!({
                "run_id": "r", "captured_at": "2026-11-24 00:00:00 UTC", "records": 10,
                "entities": 0, "duplicate_records": 4, "duplicate_groups": 2,
                "largest_group": 3, "review_backlog": 1, "auto_merge_pairs": 5,
                "review_band_pairs": 6, "duplicate_rate": 0.4, "records_per_entity": null,
            }),
            v(&InsightPoint {
                run_id: "r".into(),
                captured_at: "2026-11-24 00:00:00 UTC".into(),
                records: 10,
                entities: 0,
                duplicate_records: 4,
                duplicate_groups: 2,
                largest_group: 3,
                review_backlog: 1,
                auto_merge_pairs: 5,
                review_band_pairs: 6,
                duplicate_rate: Some(0.4),
                records_per_entity: None,
            })
        );
        assert_eq!(
            json!({
                "run_id": "r", "source_system": "crm",
                "captured_at": "2026-11-24 00:00:00 UTC", "records": 0,
                "duplicate_records": 0, "contributed_groups": 0, "duplicate_rate": null,
            }),
            v(&SourceInsight {
                run_id: "r".into(),
                source_system: "crm".into(),
                captured_at: "2026-11-24 00:00:00 UTC".into(),
                records: 0,
                duplicate_records: 0,
                contributed_groups: 0,
                duplicate_rate: None,
            })
        );
    }

    #[test]
    fn the_untagged_golden_row_is_the_bare_record() {
        let person = PersonGolden {
            id: "e1".into(),
            given_name: None,
            family_name: None,
            email: None,
            phone_e164: None,
            addr_number: None,
            addr_street: None,
            addr_unit: None,
            addr_city: None,
            addr_region: None,
            addr_postal: None,
            birth_date: None,
            metadata: None,
            survivorship_version: "v1".into(),
        };
        assert_eq!(
            serde_json::to_value(&person).unwrap(),
            serde_json::to_value(GoldenRow::Person(person)).unwrap()
        );
    }

    #[test]
    fn a_member_is_its_row_plus_the_two_flags() {
        let row = er_entities::entity_membership::read::ByEntityRow {
            source_system: "crm".into(),
            source_record_id: "1".into(),
            record_key: "crm:1".into(),
            assigned_at: at(),
            run_id: "run".into(),
        };
        // The previous build: the row's value with both flags inserted.
        let mut old = serde_json::to_value(&row).unwrap();
        old["protected"] = json!(true);
        old["verified"] = json!(false);
        let member = EntityMember {
            source_system: row.source_system,
            source_record_id: row.source_record_id,
            record_key: row.record_key,
            assigned_at: row.assigned_at,
            run_id: row.run_id,
            protected: true,
            verified: false,
        };
        assert_eq!(old, serde_json::to_value(member).unwrap());
    }
}

#[cfg(test)]
mod missing_relation_tests {
    use super::*;

    #[test]
    fn only_the_expected_missing_relation_is_optional() {
        let absent = "Catalog Error: Table with name entity_membership does not exist!";
        assert!(is_missing_table(absent, "entity_membership"));
        assert!(!is_missing_table(absent, "review_queue"));
        assert!(!is_missing_table(
            "Catalog Error: Scalar Function with name missing_fn does not exist!",
            "entity_membership"
        ));
        assert!(!is_missing_table(
            "Binder Error: Catalog \"lake\" does not exist!",
            "entity_membership"
        ));
    }

    #[test]
    fn optional_overlay_keeps_absent_relation_empty_but_propagates_catalog_faults() {
        let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
        session.execute_batch("ATTACH ':memory:' AS lake").unwrap();
        assert_eq!(active_protect_overlay(&session), Ok(false));

        session
            .execute_batch(
                "CREATE TABLE \"lake\".\"main\".\"dependency\" (id VARCHAR); \
                 CREATE VIEW \"lake\".\"main\".\"record_overlays\" AS SELECT * FROM \"lake\".\"main\".\"dependency\"; \
                 DROP TABLE \"lake\".\"main\".\"dependency\";",
            )
            .unwrap();
        let error = active_protect_overlay(&session).unwrap_err();
        assert!(error.contains("dependency"), "{error}");
    }
}

/// Page typed statement rows fetched with `limit + 1`: serialize the kept page,
/// and derive the next keyset cursor from the last kept row's key when a row
/// crossed the limit.
fn typed_page<T: serde::Serialize>(
    rows: &[T],
    snapshot: i64,
    limit: i64,
    key: impl Fn(&T) -> &str,
) -> (Vec<Value>, Option<String>) {
    let has_more = rows.len() as i64 > limit;
    let page = &rows[..rows.len().min(usize::try_from(limit).unwrap_or(0))];
    let items = rows_to_values(page);
    let next = has_more
        .then(|| page.last().map(|r| encode_cursor(snapshot, key(r))))
        .flatten();
    (items, next)
}

/// How a review page is narrowed.
///
/// A struct rather than four positional arguments because two of them are
/// `Option<&str>` and two are `Option<f64>`, and at a call site that reads as
/// noise — clippy says so about the arity too. `status` and `reason` have been
/// declared as passthrough query parameters by the BFF since the inbox shipped
/// and were accepted and IGNORED by the server, which is the defect class this
/// program keeps finding.
#[derive(Default, Clone, Copy)]
pub struct ReviewFilters<'a> {
    pub status: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub min_probability: Option<f64>,
    pub max_probability: Option<f64>,
}

/// The open-review queue, FIFO by `id` (`readapi.reviews_list`).
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn reviews_list(
    session: &Session,
    snapshot: i64,
    after: Option<&str>,
    filters: ReviewFilters<'_>,
    limit: i64,
) -> Result<(Vec<ReviewItem>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    match er_entities::review::read::page(
        session.connection(),
        snapshot,
        after,
        filters.status,
        filters.reason,
        filters.min_probability,
        filters.max_probability,
        limit + 1,
    ) {
        Ok(rows) => {
            let (rows, next) = keyset_page(rows, snapshot, limit, |r| &r.id);
            Ok((rows.into_iter().map(ReviewItem::from).collect(), next))
        }
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "review_queue") {
                Ok((Vec::new(), None))
            } else {
                Err(s)
            }
        }
    }
}

/// One review by id (`readapi.review_get`), `None` if absent / table missing.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn review_get(
    session: &Session,
    snapshot: i64,
    review_id: &str,
) -> Result<Option<er_entities::review::read::ByIdRow>, String> {
    match er_entities::review::read::by_id(session.connection(), snapshot, review_id) {
        Ok(rows) => Ok(rows.into_iter().next()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "review_queue") {
                Ok(None)
            } else {
                Err(s)
            }
        }
    }
}

/// What [`merge_group`] found for a review.
pub enum MergeGroup {
    /// The review exists but names no record pair (an entity-subject review) —
    /// the dialog has nothing to group.
    NoPair,
    /// The union of both sides' memberships as records-by-key items, each
    /// stamped with its `entity_id` (null for a record with no membership row).
    /// `total` is the union's size before the [`MAX_KEYS`] cap truncated it.
    Group { items: Vec<Value>, total: usize },
}

/// The review merge dialog's group read (S2.8): the union of BOTH sides'
/// entity memberships at `snapshot`, rec_a's entity first, each item carrying
/// its values (the same shape `records_by_key` returns) plus the `entity_id`
/// it belongs to. A side with no membership row contributes only itself.
///
/// The pair alone is not the group: when one side already sits in a two-member
/// entity, the merge the steward is deciding on combines THREE records, and a
/// dialog that showed two would edit a merge that is not the one that lands.
///
/// `Ok(None)` = no such review. [`MergeGroup::NoPair`] maps to the handler's
/// 422, mirroring the compare grid's entity-review early return.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn merge_group(
    session: &Session,
    snapshot: i64,
    review_id: &str,
) -> Result<Option<MergeGroup>, String> {
    let rows = match er_entities::review::read::by_id(session.connection(), snapshot, review_id) {
        Ok(rows) => rows,
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "review_queue") {
                return Ok(None);
            }
            return Err(s);
        }
    };
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let (Some(rec_a), Some(rec_b)) = (row.rec_a_key, row.rec_b_key) else {
        return Ok(Some(MergeGroup::NoPair));
    };
    let mut order: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut entity_of = std::collections::HashMap::<String, String>::new();
    for side in [&rec_a, &rec_b] {
        match entity_of_record_key(session, snapshot, side)? {
            Some(entity_id) => {
                for key in entity_member_keys(session, snapshot, &entity_id)? {
                    entity_of.insert(key.clone(), entity_id.clone());
                    if seen.insert(key.clone()) {
                        order.push(key);
                    }
                }
            }
            None => {
                if seen.insert(side.clone()) {
                    order.push(side.clone());
                }
            }
        }
    }
    let total = order.len();
    order.truncate(MAX_KEYS);
    let mut items = records_by_key(session, snapshot, &order)?;
    for item in &mut items {
        if let Value::Object(map) = item {
            let entity = map
                .get("record_key")
                .and_then(Value::as_str)
                .and_then(|key| entity_of.get(key));
            map.insert(
                "entity_id".to_string(),
                entity.map_or(Value::Null, |id| json!(id)),
            );
        }
    }
    Ok(Some(MergeGroup::Group { items, total }))
}

/// The assertion ledger, keyed by `id` (`readapi.assertions_list`).
/// Active-only unless `include_retracted`.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn assertions_list(
    session: &Session,
    snapshot: i64,
    include_retracted: bool,
    after: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    match er_entities::assertion::read::page(
        session.connection(),
        snapshot,
        include_retracted,
        after,
        limit + 1,
    ) {
        Ok(rows) => Ok(typed_page(&rows, snapshot, limit, |r| &r.id)),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "assertions") {
                Ok((Vec::new(), None))
            } else {
                Err(s)
            }
        }
    }
}

/// One ingest-batch receipt: what a delivery did to the corpus.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct ImportReceipt {
    pub id: String,
    pub run_id: String,
    pub source_system: String,
    pub path: String,
    pub new_count: i64,
    pub changed_count: i64,
    pub unchanged_count: i64,
    pub tombstone_count: i64,
    pub resurrected_count: i64,
    pub full_refresh_keys: bool,
    pub matched_count: Option<i64>,
    pub review_count: Option<i64>,
    pub unmatched_count: Option<i64>,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp_opt")]
    #[schema(value_type = Option<String>)]
    pub screened_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp")]
    #[schema(value_type = String)]
    pub created_at: chrono::DateTime<chrono::Utc>,
}
item_from_row!(ImportReceipt, er_entities::ingest_batch::read::RecentRow, {
    id, run_id, source_system, path, new_count, changed_count, unchanged_count,
    tombstone_count, resurrected_count, full_refresh_keys, matched_count,
    review_count, unmatched_count, screened_at, created_at,
});

/// The most recent ingest-batch receipts, newest first, capped like
/// [`runs_list`]: a bare array, not a keyset page. Optional `source` filter.
/// Empty if the `ingest_batches` table is absent.
///
/// Why this exists next to [`imports_list`]. That one is a FORWARD keyset
/// cursor — `id > $after`, `ORDER BY id` — so it answers the *oldest* page
/// first, which is correct for walking a history and wrong for a latest-activity
/// panel. The "did my file do anything?" receipts card read it with `?limit=20`,
/// so once a tenant had 20 ingest batches a newly delivered source could never
/// appear on the page that exists to answer whether the delivery did anything.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn imports_recent(
    session: &Session,
    snapshot: i64,
    source: Option<&str>,
    limit: i64,
) -> Result<Vec<ImportReceipt>, String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let source = source.filter(|s| !s.is_empty());
    match er_entities::ingest_batch::read::recent(session.connection(), snapshot, source, limit) {
        Ok(rows) => Ok(rows.into_iter().map(ImportReceipt::from).collect()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "ingest_batches") {
                Ok(Vec::new())
            } else {
                Err(s)
            }
        }
    }
}

/// The ingest-batch receipts, keyed by `id`
/// (`readapi.ingest_batches_list`). Optional `source` filter.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn imports_list(
    session: &Session,
    snapshot: i64,
    source: Option<&str>,
    after: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let source = source.filter(|s| !s.is_empty());
    match er_entities::ingest_batch::read::page(
        session.connection(),
        snapshot,
        source,
        after,
        limit + 1,
    ) {
        Ok(rows) => Ok(typed_page(&rows, snapshot, limit, |r| &r.id)),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "ingest_batches") {
                Ok((Vec::new(), None))
            } else {
                Err(s)
            }
        }
    }
}

/// One pipeline run in the run ledger, newest first.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct RunItem {
    pub id: String,
    /// `full`, `incremental`, `correct`, …
    pub mode: String,
    pub status: String,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp")]
    #[schema(value_type = String)]
    pub started_at: chrono::DateTime<chrono::Utc>,
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp_opt")]
    #[schema(value_type = Option<String>)]
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub config_hash: String,
    pub model_version: Option<String>,
    pub tf_snapshot_id: Option<String>,
    pub rebuild_reason: Option<String>,
    /// The lake snapshot holding this run's results: pass it as `snapshot` to
    /// a result read to read THIS run's output after a newer run has moved
    /// the lake. Null while the run is unfinished, and for runs finished
    /// before #1229, which never recorded one.
    pub snapshot: Option<i64>,
    /// Pairs this run scored at or above the auto-merge threshold. Absent
    /// when the dataset has no readable config, so no count is reported
    /// against the wrong threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_merged_pairs: Option<u64>,
}

impl From<er_entities::run::read::RecentRow> for RunItem {
    fn from(row: er_entities::run::read::RecentRow) -> Self {
        let er_entities::run::read::RecentRow {
            id,
            mode,
            status,
            started_at,
            ended_at,
            config_hash,
            model_version,
            tf_snapshot_id,
            rebuild_reason,
            snapshot_end,
        } = row;
        Self {
            id,
            mode,
            status,
            started_at,
            ended_at,
            config_hash,
            model_version,
            tf_snapshot_id,
            rebuild_reason,
            snapshot: snapshot_end,
            auto_merged_pairs: None,
        }
    }
}

/// One entity event a run emitted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct RunEventItem {
    pub id: String,
    /// The run-local order; the page cursor.
    pub seq: i64,
    pub run_id: String,
    pub entity_id: String,
    pub event_type: String,
    pub details: Option<Value>,
    /// Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339.
    #[serde(serialize_with = "er_entities::batch::ser_duckdb_timestamp")]
    #[schema(value_type = String)]
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}
item_from_row!(RunEventItem, er_entities::entity_event::read::ByRunRow, {
    id, seq, run_id, entity_id, event_type, details, occurred_at,
});

/// The most recent runs, newest first, capped at 20 (`readapi.runs_list`): a
/// bare array (not a keyset page). Empty if the `runs` table is absent.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn runs_list(
    session: &Session,
    snapshot: i64,
    auto_merge: Option<f64>,
) -> Result<Vec<RunItem>, String> {
    let mut runs: Vec<RunItem> =
        match er_entities::run::read::recent(session.connection(), snapshot) {
            Ok(rows) => rows.into_iter().map(RunItem::from).collect(),
            Err(e) => {
                let s = e.to_string();
                return if is_missing_table(&s, "runs") {
                    Ok(Vec::new())
                } else {
                    Err(s)
                };
            }
        };
    // Per-run auto-merge count (PR 1.5b): pairs this run scored at or above the
    // auto-merge threshold. `match_scores.run_id` is the pipeline run that scored
    // them — a full/incremental run shares one run_id across match + reconcile
    // (`run_all`), so the count attributes to the correct ledger row. Omitted when
    // the threshold is unknown (org has no readable config) so we never report a
    // count against the wrong threshold; tolerant of a lake that never matched.
    if let Some(threshold) = auto_merge {
        let counts = run_auto_merge_counts(session, snapshot, threshold)?;
        for run in &mut runs {
            run.auto_merged_pairs = Some(counts.get(&run.id).copied().unwrap_or(0));
        }
    }
    Ok(runs)
}

/// `run_id -> count of active `match_scores` at or above `auto_merge`` for that
/// run, pinned to `snapshot`. Empty when `match_scores` is absent (a lake that
/// never matched), so the run list still renders with a zero count.
fn run_auto_merge_counts(
    session: &Session,
    snapshot: i64,
    auto_merge: f64,
) -> Result<std::collections::BTreeMap<String, u64>, String> {
    match er_entities::match_score::read::auto_merge_counts(
        session.connection(),
        snapshot,
        auto_merge,
    ) {
        Ok(rows) => Ok(rows
            .into_iter()
            .filter_map(|r| {
                let n = u64::try_from(r.n?).ok()?;
                Some((r.run_id, n))
            })
            .collect()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "match_scores") {
                Ok(std::collections::BTreeMap::new())
            } else {
                Err(s)
            }
        }
    }
}

/// Merge plans for entities touched by a merge/member-add (optionally since a
/// timestamp), one row per entity with >=2 members: the elected master record
/// (per `policy`) and the member list (`readapi.merge_plans`). Reads live (the
/// latest lake state), matching the Python export.
///
/// `source_ranks` is required only by the `source_priority` policy. Returns the
/// plan rows as JSON objects.
///
/// # Errors
/// [`er_golden::master::MasterElectionError`] text for an unknown policy /
/// missing ranks, or the lake query error.
pub fn merge_plans(
    session: &Session,
    policy: &str,
    since: Option<&str>,
    source_ranks: Option<&std::collections::BTreeMap<String, i64>>,
    limit: i64,
) -> Result<Vec<MergePlan>, String> {
    use arrow::datatypes::DataType;
    use er_entities::batch;

    let limit = limit.clamp(1, MAX_LIMIT);
    let conn = session.connection();
    let since = since.filter(|s| !s.is_empty());

    // Protect enforcement (PR 1.3b): a member record with an active `protect`
    // overlay forces itself master for its entity, overriding the policy
    // election. Gated on the overlay table existing AND holding at least one
    // active protect row, so when no records are protected the generator emits
    // today's exact single-CTE masters topology.
    let protect_active = active_protect_overlay(session)?;

    // The governed generator (plan-vector-tested in er-sql-check): one flat row
    // per member, ordered by (entity_id, record_key); grouped here.
    let sql = er_golden::merge_plans_sql(
        policy,
        "lake.main",
        source_ranks,
        protect_active,
        since.is_some(),
    )
    .map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let batches: Vec<arrow::record_batch::RecordBatch> = match since {
        Some(s) => stmt.query_arrow(duckdb::params![s, limit]),
        None => stmt.query_arrow(duckdb::params![limit]),
    }
    .map_err(|e| e.to_string())?
    .collect();

    let expected = [
        ("entity_id", DataType::Utf8),
        ("master_key", DataType::Utf8),
        ("member_count", DataType::Int64),
        ("source_system", DataType::Utf8),
        ("source_record_id", DataType::Utf8),
        ("record_key", DataType::Utf8),
        ("given_name", DataType::Utf8),
        ("family_name", DataType::Utf8),
        ("email", DataType::Utf8),
        ("phone_e164", DataType::Utf8),
    ];
    let mut items: Vec<MergePlan> = Vec::new();
    let mut current: Option<MergePlan> = None;
    let err = |e: duckdb::Error| e.to_string();
    for b in &batches {
        batch::check_columns(b, &expected).map_err(err)?;
        let entity_id: &arrow::array::StringArray = batch::col(b, 0, "entity_id").map_err(err)?;
        let master_key: &arrow::array::StringArray = batch::col(b, 1, "master_key").map_err(err)?;
        let member_count: &arrow::array::Int64Array =
            batch::col(b, 2, "member_count").map_err(err)?;
        let source_system: &arrow::array::StringArray =
            batch::col(b, 3, "source_system").map_err(err)?;
        let source_record_id: &arrow::array::StringArray =
            batch::col(b, 4, "source_record_id").map_err(err)?;
        let previews: Vec<&arrow::array::StringArray> = [
            ("given_name", 6),
            ("family_name", 7),
            ("email", 8),
            ("phone_e164", 9),
        ]
        .iter()
        .map(|(name, i)| batch::col(b, *i, name).map_err(err))
        .collect::<Result<_, _>>()?;
        for i in 0..b.num_rows() {
            let eid = batch::req(batch::opt_str(entity_id, i), "entity_id", i).map_err(err)?;
            let is_new = current.as_ref().is_none_or(|plan| plan.entity_id != eid);
            if is_new {
                items.extend(current.take());
                let preview = |j: usize| batch::opt_str(previews[j], i);
                current = Some(MergePlan {
                    entity_id: eid,
                    master_key: batch::opt_str(master_key, i),
                    member_count: batch::opt_i64(member_count, i),
                    given_name: preview(0),
                    family_name: preview(1),
                    email: preview(2),
                    phone_e164: preview(3),
                    member_records: Vec::new(),
                    group_confidence: None,
                    master_record_key: None,
                    member_confidences: std::collections::BTreeMap::new(),
                });
            }
            if let Some(plan) = current.as_mut() {
                plan.member_records.push(format!(
                    "{}:{}",
                    batch::req(batch::opt_str(source_system, i), "source_system", i)
                        .map_err(err)?,
                    batch::req(batch::opt_str(source_record_id, i), "source_record_id", i)
                        .map_err(err)?
                ));
            }
        }
    }
    items.extend(current.take());

    // The same group confidence the duplicates view shows, so the export and
    // the screen agree by construction. The edges pin at the latest snapshot,
    // matching this read's own unpinned current-state view of the lake.
    if let (Some(first), Some(last)) = (items.first(), items.last()) {
        let snapshot = latest_snapshot(session)?;
        let edges = edges_by_entity(session, snapshot, &first.entity_id, &last.entity_id)?;
        for plan in &mut items {
            let score = group_score_of(
                &plan.member_records,
                edges.get(&plan.entity_id).map_or(&[], Vec::as_slice),
            );
            if let Some(score) = score {
                plan.group_confidence = Some(score.group.display());
                plan.master_record_key = Some(score.master);
                plan.member_confidences = score
                    .members
                    .into_iter()
                    .map(|(k, c)| (k, c.display()))
                    .collect();
            }
        }
    }
    Ok(items)
}

/// Render audit rows as CSV (`format=csv` on the audit feeds): the projection
/// `audit_rows` sends, one row per ledger entry, `detail` JSON verbatim in its
/// own cell.
///
/// Cells are quoted AND formula-guarded: a cell starting `=`, `+`, `-` or `@`
/// gets a leading `'`, because an audit export is exactly the file an admin
/// opens in a spreadsheet, and an actor or detail string is attacker-shaped
/// input. (`merge_plans_csv` predates this and leaves the guard to the BFF's
/// csv helper; this export is also served to direct API callers, so the guard
/// belongs server-side.)
#[must_use]
pub fn audit_csv(rows: &[Value]) -> String {
    const COLUMNS: [&str; 8] = [
        "id", "at", "actor", "org", "dataset", "action", "trace_id", "detail",
    ];
    let cell = |v: Option<&Value>| -> String {
        let s = match v {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let s = if s.starts_with(['=', '+', '-', '@']) {
            format!("'{s}")
        } else {
            s
        };
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    };
    let mut out = String::from("id,at,actor,org,dataset,action,trace_id,detail\n");
    for row in rows {
        let line: Vec<String> = COLUMNS.iter().map(|c| cell(row.get(*c))).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

/// Render merge-plan rows as CSV (`readapi.merge_plans` text/csv): the
/// `;`-joined member list, else the golden preview columns.
#[must_use]
pub fn merge_plans_csv(items: &[MergePlan]) -> String {
    let mut out = String::from(
        "entity_id,master_key,member_count,member_records,golden_given_name,\
         golden_family_name,golden_email,golden_phone_e164,group_confidence\n",
    );
    let text = |v: &Option<String>| v.clone().unwrap_or_default();
    let number = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();
    let csv_cell = |s: &str| {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    for item in items {
        let row = [
            item.entity_id.clone(),
            text(&item.master_key),
            number(item.member_count),
            item.member_records.join(";"),
            text(&item.given_name),
            text(&item.family_name),
            text(&item.email),
            text(&item.phone_e164),
            number(item.group_confidence),
        ]
        .iter()
        .map(|c| csv_cell(c))
        .collect::<Vec<_>>()
        .join(",");
        out.push_str(&row);
        out.push('\n');
    }
    out
}

/// A snapshot-pinned scalar count.
/// The standardized-record count at `snapshot` via the declared statement.
fn std_record_count(session: &Session, snapshot: i64) -> Result<i64, String> {
    let rows = er_entities::int_std_record::read::count(session.connection(), snapshot)
        .map_err(|e| e.to_string())?;
    Ok(rows.first().and_then(|r| r.n).unwrap_or(0))
}

/// A dataset's connected records — its standardized source records at the
/// latest snapshot — and that snapshot: the usage unit (#518). A lake whose
/// standardized table does not exist yet (nothing ingested) holds zero.
///
/// # Errors
/// Propagates a lake error other than the table's absence.
pub fn connected_records(session: &Session, account: bool) -> Result<(i64, i64), String> {
    let snapshot = latest_snapshot(session)?;
    let counted = if account {
        er_entities::acct_int_std_record::read::count(session.connection(), snapshot)
            .map(|rows| rows.first().and_then(|r| r.n).unwrap_or(0))
    } else {
        er_entities::int_std_record::read::count(session.connection(), snapshot)
            .map(|rows| rows.first().and_then(|r| r.n).unwrap_or(0))
    };
    match counted {
        Ok(n) => Ok((snapshot, n)),
        Err(e) if e.to_string().contains("does not exist") => Ok((snapshot, 0)),
        Err(e) => Err(e.to_string()),
    }
}

/// The golden-record count at `snapshot` via the declared statement.
fn golden_record_count(session: &Session, snapshot: i64) -> Result<i64, String> {
    let rows = er_entities::golden_record::read::count(session.connection(), snapshot)
        .map_err(|e| e.to_string())?;
    Ok(rows.first().and_then(|r| r.n).unwrap_or(0))
}

/// Duplicate-group sizing at `snapshot`: `(groups, records_in_groups)`.
fn dup_group_stats(session: &Session, snapshot: i64) -> Result<(i64, i64), String> {
    let rows =
        er_entities::entity_membership::read::dup_group_stats(session.connection(), snapshot)
            .map_err(|e| e.to_string())?;
    let row = rows.first();
    Ok((
        row.and_then(|r| r.groups).unwrap_or(0),
        row.and_then(|r| r.records_in_groups).unwrap_or(0),
    ))
}

/// `GET …/metrics`: the dashboard counters, snapshot-pinned.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct Metrics {
    pub records: i64,
    pub entities: i64,
    pub duplicate_groups: i64,
    pub records_in_duplicate_groups: i64,
    pub open_reviews: i64,
    pub snapshot: i64,
}

/// Dashboard counters (`readapi.metrics`), snapshot-pinned: record/entity totals
/// and duplicate-group sizing, plus the open-review count (0 when the lake has
/// no `review_queue` yet — that table is written by the review stage, not the
/// base pipeline).
///
/// # Errors
/// Propagates a lake query error (other than a missing `review_queue`).
pub fn metrics(session: &Session, snapshot: i64) -> Result<Metrics, String> {
    let records = std_record_count(session, snapshot)?;
    let entities = golden_record_count(session, snapshot)?;
    let (duplicate_groups, records_in_dupes) = dup_group_stats(session, snapshot)?;
    // review_queue is optional in a base-pipeline lake; absence ⇒ 0 open.
    let open_reviews = er_entities::review::read::open_count(session.connection(), snapshot)
        .ok()
        .and_then(|rows| rows.first().and_then(|r| r.n))
        .unwrap_or(0);
    Ok(Metrics {
        records,
        entities,
        duplicate_groups,
        records_in_duplicate_groups: records_in_dupes,
        open_reviews,
        snapshot,
    })
}

/// Golden-record completeness: overall and per survivable column.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct Completeness {
    pub overall: f64,
    /// Non-null fraction per golden column.
    pub by_attribute: std::collections::BTreeMap<String, f64>,
}

/// `GET …/data-quality`: the composite score and its four inputs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct DataQuality {
    /// The mean of completeness and the complements of the three rates.
    pub dq_score: f64,
    pub completeness: Completeness,
    pub junk_rate: f64,
    pub stale_rate: f64,
    pub duplicate_rate: f64,
    pub records: i64,
    pub entities: i64,
    pub stale_days: i64,
    pub snapshot: i64,
}

/// Data-quality scores (`readapi.data_quality`), snapshot-pinned and READ-ONLY —
/// a fixed-metrics v1 (no config, so `config_hash`/determinism is untouched).
/// Four components, each in `[0,1]`:
///   - `completeness`: mean over the 11 survivable golden columns of the
///     non-null fraction in `golden_records`, plus the per-attribute breakdown;
///   - `junk_rate`: fraction of `int_std_records` with a present-but-invalid
///     email or phone (`email_valid`/`phone_valid` = false);
///   - `stale_rate`: fraction whose freshness `COALESCE(updated_at_source,
///     ingested_at)` is older than `stale_days`;
///   - `duplicate_rate`: records in >=2-member entities / total records.
///
/// `dq_score` is the mean of `completeness`, `1-junk_rate`, `1-stale_rate`,
/// `1-duplicate_rate`. SELECT-only at the pinned snapshot; commits nothing.
///
/// # Errors
/// Propagates a lake query error (the core tables must exist — i.e. the org has
/// had at least one run).
pub fn data_quality(
    session: &Session,
    snapshot: i64,
    stale_days: i64,
) -> Result<DataQuality, String> {
    let conn = session.connection();
    let records = std_record_count(session, snapshot)?;
    let entities = golden_record_count(session, snapshot)?;

    // Completeness: per-column non-null fraction over golden_records — the 11
    // survivable columns, each a named count in the declared statement.
    let comp_rows = er_entities::golden_record::read::completeness_counts(conn, snapshot)
        .map_err(|e| e.to_string())?;
    let comp = comp_rows
        .first()
        .ok_or_else(|| "completeness query returned no row".to_string())?;
    let n_golden = comp.n.unwrap_or(0) as f64;
    let counts: [(&str, Option<i64>); 11] = [
        ("given_name", comp.c_given_name),
        ("family_name", comp.c_family_name),
        ("email", comp.c_email),
        ("phone_e164", comp.c_phone_e164),
        ("addr_number", comp.c_addr_number),
        ("addr_street", comp.c_addr_street),
        ("addr_unit", comp.c_addr_unit),
        ("addr_city", comp.c_addr_city),
        ("addr_region", comp.c_addr_region),
        ("addr_postal", comp.c_addr_postal),
        ("birth_date", comp.c_birth_date),
    ];
    let mut by_attribute = std::collections::BTreeMap::new();
    let mut comp_sum = 0.0;
    for (name, count) in counts {
        let cnt = count.unwrap_or(0) as f64;
        let frac = if n_golden > 0.0 { cnt / n_golden } else { 0.0 };
        comp_sum += frac;
        by_attribute.insert(name.to_string(), frac);
    }
    let completeness = comp_sum / counts.len() as f64;

    // Junk + stale in one scan of int_std_records (the stale window crosses as
    // a bound `$stale_days * INTERVAL '1 day'`, no longer inside a string).
    let js_rows = er_entities::int_std_record::read::junk_stale_counts(conn, snapshot, stale_days)
        .map_err(|e| e.to_string())?;
    let js = js_rows
        .first()
        .ok_or_else(|| "junk/stale query returned no row".to_string())?;
    let n_std = js.n.unwrap_or(0) as f64;
    let junk = js.junk.unwrap_or(0) as f64;
    let stale = js.stale.unwrap_or(0) as f64;
    let junk_rate = if n_std > 0.0 { junk / n_std } else { 0.0 };
    let stale_rate = if n_std > 0.0 { stale / n_std } else { 0.0 };

    // Duplicate rate: records in >=2-member entities / total records.
    let (_, records_in_dupes) = dup_group_stats(session, snapshot)?;
    let duplicate_rate = if records > 0 {
        records_in_dupes as f64 / records as f64
    } else {
        0.0
    };

    let dq_score =
        (completeness + (1.0 - junk_rate) + (1.0 - stale_rate) + (1.0 - duplicate_rate)) / 4.0;

    Ok(DataQuality {
        dq_score,
        completeness: Completeness {
            overall: completeness,
            by_attribute,
        },
        junk_rate,
        stale_rate,
        duplicate_rate,
        records,
        entities,
        stale_days,
        snapshot,
    })
}

/// One keyset page of a run's entity events, oldest first — the detail behind
/// the `entity.*` webhook aggregate (O3c-2).
///
/// The aggregate names a count and a `detail_path`; this is what that path
/// serves. Cursoring on `seq` rather than `id` because `seq` is what the ledger
/// orders by: `id` is a content hash, so an `id` cursor would page in an order
/// no reader asked for.
///
/// `after` arrives through the standard cursor, whose key is a string, so it is
/// parsed back to the `seq` it encodes. A cursor whose key is not a number
/// yields no page rather than an error — it can only come from a cursor this
/// function did not mint.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn entity_events_by_run(
    session: &Session,
    snapshot: i64,
    run_id: &str,
    event_type: Option<&str>,
    after: Option<&str>,
    limit: i64,
) -> Result<(Vec<RunEventItem>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let after_seq = match after {
        Some(raw) => match raw.parse::<i64>() {
            Ok(n) => Some(n),
            Err(_) => return Ok((Vec::new(), None)),
        },
        None => None,
    };
    match er_entities::entity_event::read::by_run(
        session.connection(),
        snapshot,
        run_id,
        event_type,
        after_seq,
        limit + 1,
    ) {
        Ok(rows) => {
            let has_more = rows.len() as i64 > limit;
            let mut rows = rows;
            rows.truncate(usize::try_from(limit).unwrap_or(0));
            let next = has_more
                .then(|| {
                    rows.last()
                        .map(|r| encode_cursor(snapshot, &r.seq.to_string()))
                })
                .flatten();
            Ok((rows.into_iter().map(RunEventItem::from).collect(), next))
        }
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "entity_events") {
                Ok((Vec::new(), None))
            } else {
                Err(s)
            }
        }
    }
}

/// The entity a record currently belongs to at `snapshot`, or `None` when the
/// corpus no longer has it (or membership has not been materialized).
///
/// The write-back handler's lookup (O4): a resolution names a PAIR of record
/// keys, and what the tenant's system needs back is the master those records
/// now resolve to — one entity after a `match`, two after a `no_match`.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn entity_of_record_key(
    session: &Session,
    snapshot: i64,
    record_key: &str,
) -> Result<Option<String>, String> {
    match er_entities::entity_membership::read::entity_of_key(
        session.connection(),
        snapshot,
        record_key,
    ) {
        Ok(rows) => Ok(rows.into_iter().next().map(|r| r.entity_id)),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "entity_membership") {
                Ok(None)
            } else {
                Err(s)
            }
        }
    }
}

/// How many OPEN reviews carry no match probability at `snapshot`.
///
/// The `never_unsatisfiable` escalations, which a confidence band necessarily
/// excludes — they are not scored pairs but structural contradictions, so they
/// are in no band. Counted so the inbox can SAY how many items a band is
/// hiding: an unsatisfiable constraint is a question for a human, and a filter
/// that silently removes it is how it goes unanswered forever.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn unscored_open_reviews(session: &Session, snapshot: i64) -> Result<i64, String> {
    match er_entities::review::read::unscored_open_count(session.connection(), snapshot) {
        Ok(rows) => Ok(rows.first().and_then(|r| r.n).unwrap_or(0)),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "review_queue") {
                Ok(0)
            } else {
                Err(s)
            }
        }
    }
}

/// One source's share of a run's duplication.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct SourceInsight {
    pub run_id: String,
    pub source_system: String,
    /// chrono's display form (`YYYY-MM-DD HH:MM:SS[.f] UTC`).
    pub captured_at: String,
    pub records: i64,
    pub duplicate_records: i64,
    pub contributed_groups: i64,
    /// Null when the source contributed no records.
    pub duplicate_rate: Option<f64>,
}

/// One run's per-source duplicate attribution, worst first (O7b-2).
///
/// The corpus-wide duplicate rate says a tenant has a problem; it does not say
/// WHERE the problem comes from, and the answer is nearly always one feed.
/// Rates are derived on read rather than stored, the same rule the corpus-wide
/// series follows — and null rather than 0.0 for a source with no records,
/// because no records is not a 0% duplicate rate, it is no answer.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn insight_sources(
    session: &Session,
    snapshot: i64,
    run_id: &str,
) -> Result<Vec<SourceInsight>, String> {
    match er_entities::metric_snapshot_source::read::by_run(session.connection(), snapshot, run_id)
    {
        Ok(rows) => Ok(rows
            .iter()
            .map(|r| {
                let rate = if r.records > 0 {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a source large enough to lose f64 precision here is far past \
                                  what a rate reported to three decimals can express"
                    )]
                    Some(r.duplicate_records as f64 / r.records as f64)
                } else {
                    None
                };
                SourceInsight {
                    run_id: r.run_id.clone(),
                    source_system: r.source_system.clone(),
                    captured_at: r.captured_at.to_string(),
                    records: r.records,
                    duplicate_records: r.duplicate_records,
                    contributed_groups: r.contributed_groups,
                    duplicate_rate: rate,
                }
            })
            .collect()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s, "metric_snapshot_sources") {
                Ok(Vec::new())
            } else {
                Err(s)
            }
        }
    }
}

/// What the latest standardized profile says about the columns a config blocks
/// on, for the publish-time advisories.
///
/// Tier 1, because that is what every run writes automatically; the advisories
/// need fill rates and value shares, neither of which tier 2 adds.
///
/// # Errors
/// Propagates the lake read. The CALLER is expected to treat an error as "no
/// advice" rather than as a failed publish — a publish must not depend on the
/// profiler having run, and an advisory that can refuse a publish is a gate
/// wearing a suggestion's clothes.
pub fn profiled_standardized_columns(
    session: &Session,
    snapshot: i64,
) -> Result<Vec<crate::config_check::ProfiledColumn>, String> {
    let runs = er_entities::profile_run::read::latest(
        session.connection(),
        snapshot,
        "standardized",
        er_cli::profile::NO_SOURCE,
        1,
    )
    .map_err(|e| e.to_string())?;
    let Some(run) = runs.first() else {
        return Ok(Vec::new());
    };
    let columns =
        er_entities::column_profile::read::for_run(session.connection(), snapshot, &run.id)
            .map_err(|e| e.to_string())?;
    Ok(columns
        .iter()
        .map(|c| crate::config_check::ProfiledColumn {
            column_name: c.column_name.clone(),
            row_count: c.row_count,
            non_null_count: c.non_null_count,
            blank_count: c.blank_count,
            // EXACT only. The duplicate floor is a record count, and deriving
            // one from `distinct_approx` would turn a HyperLogLog's error —
            // wrong in both directions — into a number a tenant reads as "you
            // have N duplicates".
            distinct_exact: c.distinct_exact,
            // The profile stores the top values as JSON; only the leading
            // share is wanted here, and reaching past it would pull a real
            // value into the publish path for no reason.
            top_share: c
                .top_values
                .as_ref()
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|first| first.get("share"))
                .and_then(serde_json::Value::as_f64),
        })
        .collect())
}

/// One profiling finding; `column_name` is null for a corpus-level one.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct ProfileFinding {
    pub code: String,
    pub severity: String,
    pub column_name: Option<String>,
    pub message: String,
    pub detail: Option<Value>,
}

/// One measured candidate blocking key, cheapest first.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct CandidateKeyItem {
    pub key_columns: Vec<String>,
    pub row_count: i64,
    pub covered_rows: i64,
    pub distinct_values: i64,
    pub expected_pairs: i64,
    pub largest_block_pairs: i64,
    pub largest_block_rows: i64,
    pub entropy_bits: f64,
    pub pair_loss_from_nulls: f64,
}

/// The latest profile of one corpus stage.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct ProfileReport {
    /// Always true.
    pub profiled: bool,
    pub profile_run_id: String,
    pub subject: String,
    pub source_system: String,
    pub tier: i64,
    pub data_snapshot: i64,
    pub corpus_digest: String,
    pub row_count: i64,
    pub column_count: i64,
    pub ingest_batch_id: Option<String>,
    pub config_hash: Option<String>,
    pub model_version: Option<String>,
    /// Whether the figures come from a sample (then `sample_proportion` says
    /// how much of the corpus it covered).
    pub sampled: bool,
    pub sample_proportion: Option<f64>,
    pub sample_seed: Option<i64>,
    pub duration_ms: i64,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    /// One generated column-profile row per column. Value-bearing fields
    /// (top values, examples) are redacted below Steward.
    pub columns: Vec<Value>,
    pub findings: Vec<ProfileFinding>,
    /// Empty for a tier-1 run, which does not measure keys.
    pub candidate_keys: Vec<CandidateKeyItem>,
}

/// Nothing has profiled this subject and tier yet.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct ProfileNotYet {
    /// Always false.
    pub profiled: bool,
    pub subject: String,
    pub tier: i64,
}

/// `GET …/profile`: the latest profile, or that there is none yet.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum Profile {
    Report(Box<ProfileReport>),
    NotYet(ProfileNotYet),
}

/// One corpus stage's profile: the run, every column's shape, and the findings.
///
/// `include_values` gates every field that echoes a value **as stored** — see
/// [`VALUE_BEARING_COLUMN_FIELDS`]. A viewer gets the statistics with those
/// stripped; a steward gets them intact. The decision is made here rather than
/// in the statement because it is about WHO is asking, which SQL does not know.
///
/// This gate first shipped covering `top_values` alone, described in this very
/// comment as "the one field in a profile that carries personal data". It was
/// not: `lex_min`/`lex_max` are the lexically smallest and largest values in the
/// column, and `num_min`/`num_max` the numeric extremes — all four verbatim
/// data, so on an email column `lex_min` is a real address and on a salary
/// column `num_max` is one person's pay. A gate covering one of five fields is
/// worse than no gate, because it reads as complete. The classification is now
/// a pair of exhaustive lists that
/// `tests/profile_value_gate.rs` checks against the entity definition, so a
/// column added to `column_profile` cannot reach the wire unclassified.
///
/// A corpus that has never been profiled reads as `{"profiled": false}` rather
/// than erroring, on the same principle as `quality`'s `configured: false`:
/// asking is a fair question whether or not the answer exists yet.
///
/// # Errors
/// Propagates the lake read.
pub fn profile(
    session: &Session,
    snapshot: i64,
    subject: &str,
    source_system: &str,
    tier: i64,
    include_values: bool,
) -> Result<Profile, String> {
    let not_yet = || {
        Profile::NotYet(ProfileNotYet {
            profiled: false,
            subject: subject.to_string(),
            tier,
        })
    };
    let runs = match er_entities::profile_run::read::latest(
        session.connection(),
        snapshot,
        subject,
        source_system,
        tier,
    ) {
        Ok(rows) => rows,
        // The relation is absent until something profiles, and "not yet" is the
        // honest answer to the question rather than a failure.
        Err(e) if is_missing_relation(&e.to_string()) => return Ok(not_yet()),
        Err(e) => return Err(e.to_string()),
    };
    let Some(run) = runs.first() else {
        return Ok(not_yet());
    };

    let columns =
        er_entities::column_profile::read::for_run(session.connection(), snapshot, &run.id)
            .map_err(|e| e.to_string())?;
    let findings =
        er_entities::profile_finding::read::for_run(session.connection(), snapshot, &run.id)
            .map_err(|e| e.to_string())?;
    // Only a tier-2 run measures candidate keys, so an absent set is the normal
    // tier-1 answer rather than a gap — it travels as an empty array, which is
    // what "measured nothing" looks like to a consumer that renders a table.
    let candidate_keys =
        er_entities::candidate_key::read::for_run(session.connection(), snapshot, &run.id)
            .map_err(|e| e.to_string())?;

    Ok(Profile::Report(Box::new(ProfileReport {
        profiled: true,
        profile_run_id: run.id.clone(),
        subject: run.subject.clone(),
        source_system: run.source_system.clone(),
        tier: run.tier,
        data_snapshot: run.snapshot,
        corpus_digest: run.corpus_digest.clone(),
        row_count: run.row_count,
        column_count: run.column_count,
        ingest_batch_id: run.ingest_batch_id.clone(),
        config_hash: run.config_hash.clone(),
        model_version: run.model_version.clone(),
        // `sampled` travels with the proportion so a consumer rendering a
        // number knows whether to label it approximate; a figure from a 5%
        // sample shown as exact is worse than no figure.
        sampled: run.sampled,
        sample_proportion: run.sample_proportion,
        sample_seed: run.sample_seed,
        duration_ms: run.duration_ms,
        captured_at: run.captured_at,
        // Serialize the generated row rather than restating its 26 fields: a
        // hand-written struct here would be one more place to forget a column
        // the entity gained. The gated fields are patched afterwards.
        columns: columns
            .iter()
            .map(|c| {
                let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
                if let Some(obj) = v.as_object_mut() {
                    redact_column_values(obj, include_values);
                }
                v
            })
            .collect(),
        findings: findings
            .iter()
            .map(|f| ProfileFinding {
                code: f.code.clone(),
                severity: f.severity.clone(),
                // The empty string is the stored sentinel for a corpus-level
                // finding; it becomes null on the wire so a consumer does not
                // have to know that, and cannot match a column named "".
                column_name: (!f.column_name.is_empty()).then(|| f.column_name.clone()),
                message: f.message.clone(),
                detail: f.detail.clone(),
            })
            .collect(),
        // Cheapest key first, as the statement ordered them. `key_columns` is
        // stored chr(31)-joined because the lake has no list type; it is split
        // back into an array so a consumer never has to know that encoding.
        candidate_keys: candidate_keys
            .iter()
            .map(|k| CandidateKeyItem {
                key_columns: k.key_columns.split('\u{1f}').map(str::to_string).collect(),
                row_count: k.row_count,
                covered_rows: k.covered_rows,
                distinct_values: k.distinct_values,
                expected_pairs: k.expected_pairs,
                largest_block_pairs: k.largest_block_pairs,
                largest_block_rows: k.largest_block_rows,
                entropy_bits: k.entropy_bits,
                pair_loss_from_nulls: k.pair_loss_from_nulls,
            })
            .collect(),
    })))
}

/// Fields of a `column_profile` row that echo a value **as stored**, and so
/// carry whatever the corpus carries — a real address, a real name, one
/// person's salary.
///
/// The test of membership is "could a reader recover a datum from this field",
/// not the column's type: `num_max` is a number and belongs here, `num_avg` is
/// a number and does not, because an average of many rows is nobody's value.
/// Drawing the line at verbatim-or-not keeps it one sentence long; drawing it
/// at "which types feel sensitive" is how `lex_min` was missed the first time.
pub const VALUE_BEARING_COLUMN_FIELDS: [&str; 5] =
    ["top_values", "lex_min", "lex_max", "num_min", "num_max"];

/// Fields of a `column_profile` row that are counts, lengths, or statistics
/// derived from many rows — safe for any role that may see the column exists.
///
/// Listed exhaustively rather than inferred as "everything else" so that a
/// column added to the entity lands in NEITHER list and fails the gate test,
/// which is the point: the failure asks a question that has to be answered.
pub const DERIVED_COLUMN_FIELDS: [&str; 25] = [
    "id",
    "profile_run_id",
    "column_name",
    "row_count",
    "total_row_count",
    "non_null_count",
    "blank_count",
    "distinct_approx",
    "distinct_exact",
    "distinct_normalized",
    "len_min",
    "len_max",
    "len_avg",
    "len_p50",
    "len_p95",
    "num_avg",
    "num_std",
    "cast_bigint",
    "cast_double",
    "cast_date",
    "cast_timestamp",
    "cast_boolean",
    "entropy_bits",
    "created_at",
    "updated_at",
];

/// Strip the value-bearing fields of one serialized `column_profile` row unless
/// the caller is entitled to them.
///
/// Values are nulled in place rather than removed, so the response shape does
/// not depend on who asked — a consumer renders "—" for a null it expected, and
/// throws on a key that vanished. `top_values` keeps its array and loses only
/// each entry's `value`, because the SHAPE of the distribution is what a viewer
/// needs to read a skew finding: that the top value covers 31% of rows is the
/// point, and which value it is adds nothing to that.
fn redact_column_values(obj: &mut serde_json::Map<String, Value>, include_values: bool) {
    if include_values {
        return;
    }
    for field in VALUE_BEARING_COLUMN_FIELDS {
        if field == "top_values" {
            let redacted = redact_top_values(obj.get(field));
            obj.insert(field.to_string(), redacted);
        } else if obj.contains_key(field) {
            obj.insert(field.to_string(), Value::Null);
        }
    }
}

/// `top_values` with the values removed and the counts kept.
fn redact_top_values(raw: Option<&Value>) -> Value {
    let Some(Value::Array(items)) = raw else {
        return Value::Null;
    };
    Value::Array(
        items
            .iter()
            .map(|v| {
                json!({
                    "value": Value::Null,
                    "count": v.get("count").cloned().unwrap_or(Value::Null),
                    "share": v.get("share").cloned().unwrap_or(Value::Null),
                    "rank": v.get("rank").cloned().unwrap_or(Value::Null),
                })
            })
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod value_gate_tests {
    use super::{DERIVED_COLUMN_FIELDS, VALUE_BEARING_COLUMN_FIELDS, redact_column_values};
    use serde_json::{Value, json};

    /// A column profile with every value-bearing field populated with something
    /// recognisable, so a leak shows up as the literal string in an assertion
    /// rather than as a shape mismatch.
    fn row() -> serde_json::Map<String, Value> {
        let mut obj = serde_json::Map::new();
        obj.insert("column_name".into(), json!("email"));
        obj.insert("lex_min".into(), json!("aaron@example.com"));
        obj.insert("lex_max".into(), json!("zoe@example.com"));
        obj.insert("num_min".into(), json!(18.0));
        obj.insert("num_max".into(), json!(97.0));
        obj.insert("num_avg".into(), json!(41.5));
        obj.insert("num_std".into(), json!(12.25));
        obj.insert("non_null_count".into(), json!(940));
        obj.insert("entropy_bits".into(), json!(9.5));
        obj.insert(
            "top_values".into(),
            json!([{"value": "sam@example.com", "count": 31, "share": 0.031, "rank": 1}]),
        );
        obj
    }

    #[test]
    fn a_steward_gets_the_row_untouched() {
        let before = row();
        let mut after = row();
        redact_column_values(&mut after, true);
        assert_eq!(Value::Object(after), Value::Object(before));
    }

    #[test]
    fn a_viewer_gets_no_value_bearing_field() {
        let mut obj = row();
        redact_column_values(&mut obj, false);

        for field in VALUE_BEARING_COLUMN_FIELDS {
            if field == "top_values" {
                continue;
            }
            assert_eq!(
                obj.get(field),
                Some(&Value::Null),
                "{field} reached a viewer"
            );
        }

        // Nulled in place, never removed: a key that vanishes by role makes the
        // response shape depend on who asked, and a consumer that reads it
        // throws instead of rendering "—".
        for field in VALUE_BEARING_COLUMN_FIELDS {
            assert!(obj.contains_key(field), "{field} was removed, not nulled");
        }

        // Nothing identifying survives anywhere in the serialized form — this
        // is the assertion that would have caught the original gate, because it
        // does not depend on knowing WHICH field holds the address.
        let text = Value::Object(obj.clone()).to_string();
        for leak in ["aaron@example.com", "zoe@example.com", "sam@example.com"] {
            assert!(!text.contains(leak), "{leak} survived redaction: {text}");
        }
    }

    #[test]
    fn a_viewer_keeps_every_statistic_including_the_distribution_shape() {
        let mut obj = row();
        redact_column_values(&mut obj, false);

        for field in DERIVED_COLUMN_FIELDS {
            if let Some(expected) = row().get(field) {
                assert_eq!(obj.get(field), Some(expected), "{field} should survive");
            }
        }

        // The skew is the point of the panel: counts, shares and ranks survive
        // so "the top value covers 3.1% of rows" is still readable without
        // knowing which value it is.
        let top = obj
            .get("top_values")
            .and_then(Value::as_array)
            .expect("array");
        assert_eq!(
            top.len(),
            1,
            "the entry survives rather than the array going"
        );
        assert_eq!(top[0].get("count"), Some(&json!(31)));
        assert_eq!(top[0].get("share"), Some(&json!(0.031)));
        assert_eq!(top[0].get("rank"), Some(&json!(1)));
        assert_eq!(top[0].get("value"), Some(&Value::Null));
    }
}

#[cfg(test)]
mod duplicate_group_tests {
    //! #1234: the typed duplicate groups serialize to exactly the keys and
    //! values the hand-built `into_json` map did, for both entity types and
    //! for a scored and an unscored group (equal as parsed values).
    use super::{DuplicateGroups, GroupItem};
    use dedupe_core::confidence::{Confidence, GroupScore};
    use er_config::EntityType;
    use serde_json::json;

    fn item(score: Option<GroupScore>, preview: Vec<Option<String>>) -> GroupItem {
        GroupItem {
            entity_id: "e1".into(),
            member_count: 2,
            member_keys: vec!["crm:1".into(), "erp:9".into()],
            score,
            model_versions: vec!["m1".into()],
            matched_fields: vec!["email".into()],
            verified: true,
            preview,
        }
    }

    #[test]
    fn a_scored_person_group_keeps_its_wire_shape() {
        let score = GroupScore {
            master: "crm:1".into(),
            members: vec![
                ("crm:1".into(), Confidence::from_probability(0.9)),
                ("erp:9".into(), Confidence::from_probability(0.8)),
            ],
            group: Confidence::from_probability(0.85),
        };
        let expected_group = score.group.display();
        let expected_a = score.members[0].1.display();
        let expected_b = score.members[1].1.display();
        let mut page = DuplicateGroups::new(EntityType::Person);
        page.push(item(
            Some(score),
            vec![Some("Ada".into()), None, Some("ada@example.com".into())],
        ));
        assert_eq!(
            page.to_values(),
            vec![json!({
                "entity_id": "e1",
                "member_count": 2,
                "members": ["crm:1", "erp:9"],
                "group_confidence": expected_group,
                "master_record_key": "crm:1",
                "member_confidences": {"crm:1": expected_a, "erp:9": expected_b},
                "model_versions": ["m1"],
                "matched_fields": ["email"],
                "verified": true,
                "given_name": "Ada",
                "family_name": null,
                "email": "ada@example.com",
            })]
        );
    }

    #[test]
    fn an_unscored_account_group_keeps_its_wire_shape() {
        let mut page = DuplicateGroups::new(EntityType::Account);
        page.push(item(None, vec![Some("Acme".into()), None]));
        assert_eq!(
            page.to_values(),
            vec![json!({
                "entity_id": "e1",
                "member_count": 2,
                "members": ["crm:1", "erp:9"],
                "group_confidence": null,
                "master_record_key": null,
                "member_confidences": {},
                "model_versions": ["m1"],
                "matched_fields": ["email"],
                "verified": true,
                "company_name": "Acme",
                "domain": null,
            })]
        );
    }
}

#[cfg(test)]
mod scored_pair_tests {
    //! #1234: a typed match-score item is exactly what `rows_to_values` +
    //! `with_confidence` made of its row (equal as parsed values).
    use super::*;

    #[test]
    fn a_scored_pair_is_the_row_plus_its_confidence() {
        let row = er_entities::match_score::read::ActivePageRow {
            rec_a_key: "crm:1".into(),
            rec_b_key: "erp:9".into(),
            match_probability: 0.912,
            model_version: "m1".into(),
            evidence: json!({"gamma_email": 2, "match_weight": 7.5}),
            run_id: "run1".into(),
            scored_at: chrono::DateTime::from_timestamp(1_700_000_000, 120_000_000).unwrap(),
        };
        let mut old = rows_to_values(std::slice::from_ref(&row));
        with_confidence(&mut old);
        assert_eq!(old[0], serde_json::to_value(ScoredPair::from(row)).unwrap());
    }
}

#[cfg(test)]
mod pin_requested_tests {
    //! #1229: an explicit `snapshot` is honoured exactly, or refused loudly.
    use super::{PinError, pin_requested};

    const BOUNDS: Result<(i64, i64), String> = Ok((5, 9));

    #[test]
    fn no_request_passes_the_start_through() {
        assert_eq!(pin_requested((9, None), None, || BOUNDS), Ok((9, None)));
    }

    #[test]
    fn a_retained_snapshot_is_read_exactly() {
        assert_eq!(pin_requested((9, None), Some(6), || BOUNDS), Ok((6, None)));
        assert_eq!(pin_requested((9, None), Some(5), || BOUNDS), Ok((5, None)));
    }

    #[test]
    fn an_expired_or_future_snapshot_is_refused() {
        assert_eq!(
            pin_requested((9, None), Some(4), || BOUNDS),
            Err(PinError::Expired {
                requested: 4,
                oldest: 5
            })
        );
        assert_eq!(
            pin_requested((9, None), Some(10), || BOUNDS),
            Err(PinError::NotYet {
                requested: 10,
                latest: 9
            })
        );
    }

    #[test]
    fn a_cursor_must_agree_and_needs_no_catalog_read() {
        let start = (6, Some("k".to_string()));
        let unread = || -> Result<(i64, i64), String> { panic!("bounds read for a cursor") };
        assert_eq!(
            pin_requested(start.clone(), Some(6), unread),
            Ok(start.clone())
        );
        assert_eq!(
            pin_requested(start, Some(7), unread),
            Err(PinError::CursorDisagrees {
                cursor: 6,
                requested: 7
            })
        );
    }
}
