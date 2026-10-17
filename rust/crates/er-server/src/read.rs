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
const MAX_LIMIT: i64 = 200;

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

/// A SQL string literal: single-quote the value, doubling embedded quotes.
fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
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
/// `main`).
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
    let mut session =
        Session::open_in_memory(SessionOptions::deterministic()).map_err(|e| e.to_string())?;
    session
        .attach_ducklake(&DuckLakeConfig {
            metadata: catalog.to_string(),
            data_path: data_path.map(str::to_string),
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
    use arrow::array::StringArray;
    use arrow::record_batch::RecordBatch;

    let limit = limit.clamp(1, MAX_LIMIT);
    let conn = session.connection();
    // The read statements bind `$q` as a LIKE pattern (`... ILIKE $q ESCAPE '\'`),
    // so the contains-search term is wrapped in `%…%` with its LIKE metacharacters
    // escaped here; an empty term means "no filter" (NULL).
    let q_pattern = q.filter(|s| !s.is_empty()).map(like_contains);
    let q = q_pattern.as_deref();

    // Fetch one extra row to decide whether a next page exists, via the generated
    // typed query function (person or account).
    let batches = match entity_type {
        EntityType::Person => {
            er_entities::golden_record::read::list_page(conn, snapshot, after, q, limit + 1)
        }
        EntityType::Account => {
            er_entities::acct_golden_record::read::list_page(conn, snapshot, after, q, limit + 1)
        }
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
) -> Result<Option<Value>, String> {
    let conn = session.connection();
    let golden = match entity_type {
        EntityType::Person => {
            let rows = er_entities::golden_record::read::by_id(conn, snapshot, entity_id)
                .map_err(|e| e.to_string())?;
            rows_to_values(&rows)
        }
        EntityType::Account => {
            let rows = er_entities::acct_golden_record::read::by_id(conn, snapshot, entity_id)
                .map_err(|e| e.to_string())?;
            rows_to_values(&rows)
        }
    };
    let Some(golden_row) = golden.into_iter().next() else {
        return Ok(None);
    };
    let members = er_entities::entity_membership::read::by_entity(conn, snapshot, entity_id)
        .map_err(|e| e.to_string())?;
    let lineage = match entity_type {
        EntityType::Person => {
            let rows = er_entities::golden_lineage::read::by_entity(conn, snapshot, entity_id)
                .map_err(|e| e.to_string())?;
            rows_to_values(&rows)
        }
        EntityType::Account => {
            let rows = er_entities::acct_golden_lineage::read::by_entity(conn, snapshot, entity_id)
                .map_err(|e| e.to_string())?;
            rows_to_values(&rows)
        }
    };
    let events = er_entities::entity_event::read::by_entity(conn, snapshot, entity_id)
        .map_err(|e| e.to_string())?;
    Ok(Some(json!({
        "golden": golden_row,
        "members": rows_to_values(&members),
        "lineage": lineage,
        "events": rows_to_values(&events),
    })))
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
            if is_missing_table(&s) {
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
            .map(|r| (r.assertion_id, r.rec_a_key, r.rec_b_key, r.kind))
            .collect()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s) {
                Ok(Vec::new())
            } else {
                Err(s)
            }
        }
    }
}

/// A keyset page of duplicate groups: entities with >=2 members, with their
/// member list and a golden preview (`readapi.duplicate_groups`). Keyset on
/// `entity_id`. Three declared statements — the group page, the member rows in
/// the page's id range, and the golden preview in that range — assembled here
/// (the old single query leaned on DuckDB-only `list(.. ORDER BY ..)`).
///
/// # Errors
/// Propagates the lake query error.
pub fn duplicates(
    session: &Session,
    entity_type: EntityType,
    snapshot: i64,
    after: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, Option<String>), String> {
    use std::collections::BTreeMap;

    let limit = limit.clamp(1, MAX_LIMIT);
    let conn = session.connection();
    let groups = er_entities::entity_membership::read::duplicate_groups_page(
        conn,
        snapshot,
        after,
        limit + 1,
    )
    .map_err(|e| e.to_string())?;
    let has_more = groups.len() as i64 > limit;
    let page = &groups[..groups.len().min(usize::try_from(limit).unwrap_or(0))];
    let Some((first, last)) = page.first().map(|f| {
        (
            f.entity_id.as_str(),
            page[page.len() - 1].entity_id.as_str(),
        )
    }) else {
        return Ok((Vec::new(), None));
    };

    // The members of every entity in the page's id range, already ordered by
    // (entity_id, record_key) — so each group's list is record_key-ordered, the
    // way the old `list(.. ORDER BY record_key)` aggregate built it.
    let members =
        er_entities::entity_membership::read::members_in_range(conn, snapshot, first, last)
            .map_err(|e| e.to_string())?;
    let mut member_lists: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for m in &members {
        member_lists
            .entry(m.entity_id.as_str())
            .or_default()
            .push(Value::from(format!(
                "{}:{}",
                m.source_system, m.source_record_id
            )));
    }

    // The golden preview per entity in the range (person: name+email; account:
    // company+domain), as a ready-to-insert field map. A group whose golden row
    // is absent previews as nulls, like the old LEFT JOIN.
    let preview_fields: &[&str] = match entity_type {
        EntityType::Person => &["given_name", "family_name", "email"],
        EntityType::Account => &["company_name", "domain"],
    };
    let mut previews: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    match entity_type {
        EntityType::Person => {
            let rows =
                er_entities::golden_record::read::preview_in_range(conn, snapshot, first, last)
                    .map_err(|e| e.to_string())?;
            for r in rows {
                previews.insert(
                    r.entity_id,
                    vec![
                        r.given_name.map_or(Value::Null, Value::from),
                        r.family_name.map_or(Value::Null, Value::from),
                        r.email.map_or(Value::Null, Value::from),
                    ],
                );
            }
        }
        EntityType::Account => {
            let rows = er_entities::acct_golden_record::read::preview_in_range(
                conn, snapshot, first, last,
            )
            .map_err(|e| e.to_string())?;
            for r in rows {
                previews.insert(
                    r.entity_id,
                    vec![
                        r.company_name.map_or(Value::Null, Value::from),
                        r.domain.map_or(Value::Null, Value::from),
                    ],
                );
            }
        }
    }

    let items: Vec<Value> = page
        .iter()
        .map(|g| {
            let mut obj = serde_json::Map::new();
            obj.insert("entity_id".to_string(), Value::from(g.entity_id.clone()));
            obj.insert(
                "member_count".to_string(),
                Value::from(g.member_count.unwrap_or(0)),
            );
            obj.insert(
                "members".to_string(),
                Value::Array(
                    member_lists
                        .remove(g.entity_id.as_str())
                        .unwrap_or_default(),
                ),
            );
            let values = previews.remove(&g.entity_id);
            for (i, field) in preview_fields.iter().enumerate() {
                let v = values
                    .as_ref()
                    .and_then(|vals| vals.get(i).cloned())
                    .unwrap_or(Value::Null);
                obj.insert((*field).to_string(), v);
            }
            Value::Object(obj)
        })
        .collect();
    let next_cursor = has_more.then(|| encode_cursor(snapshot, last));
    Ok((items, next_cursor))
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
) -> Result<(Vec<Value>, Option<String>), String> {
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
    let items = rows_to_values(page);
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
    Ok((items, next_cursor))
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
    // The old `struct_pack` nesting, rebuilt in Rust: the identity columns on
    // top, every other column under `attributes`. Serializing the typed row
    // first keeps the DuckDB-compatible timestamp rendering.
    Ok(rows
        .iter()
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
            top.insert("attributes".to_string(), Value::Object(all));
            Value::Object(top)
        })
        .collect())
}

/// Whether a lake query error is "the table isn't there" — expected for a lake
/// whose review/assertion/run/ingest-tracking stages haven't populated these
/// relations yet, in which case the reader returns an empty page rather than an
/// error.
fn is_missing_table(err: &str) -> bool {
    err.contains("does not exist")
        || err.contains("Catalog Error")
        || err.contains("Table with name")
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

/// The open-review queue, FIFO by `review_id` (`readapi.reviews_list`).
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn reviews_list(
    session: &Session,
    snapshot: i64,
    after: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    match er_entities::review::read::page(session.connection(), snapshot, after, limit + 1) {
        Ok(rows) => Ok(typed_page(&rows, snapshot, limit, |r| &r.review_id)),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s) {
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
) -> Result<Option<Value>, String> {
    match er_entities::review::read::by_id(session.connection(), snapshot, review_id) {
        Ok(rows) => Ok(rows_to_values(&rows).into_iter().next()),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s) {
                Ok(None)
            } else {
                Err(s)
            }
        }
    }
}

/// The assertion ledger, keyed by `assertion_id` (`readapi.assertions_list`).
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
        Ok(rows) => Ok(typed_page(&rows, snapshot, limit, |r| &r.assertion_id)),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s) {
                Ok((Vec::new(), None))
            } else {
                Err(s)
            }
        }
    }
}

/// The ingest-batch receipts, keyed by `ingest_batch_id`
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
        Ok(rows) => Ok(typed_page(&rows, snapshot, limit, |r| &r.ingest_batch_id)),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s) {
                Ok((Vec::new(), None))
            } else {
                Err(s)
            }
        }
    }
}

/// The most recent runs, newest first, capped at 20 (`readapi.runs_list`): a
/// bare array (not a keyset page). Empty if the `runs` table is absent.
///
/// # Errors
/// Propagates a non-missing-table lake query error.
pub fn runs_list(
    session: &Session,
    snapshot: i64,
    auto_merge: Option<f64>,
) -> Result<Vec<Value>, String> {
    let mut runs: Vec<Value> = match er_entities::run::read::recent(session.connection(), snapshot)
    {
        Ok(rows) => rows_to_values(&rows),
        Err(e) => {
            let s = e.to_string();
            return if is_missing_table(&s) {
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
            if let Value::Object(map) = run {
                if let Some(id) = map.get("run_id").and_then(Value::as_str) {
                    let n = counts.get(id).copied().unwrap_or(0);
                    map.insert("auto_merged_pairs".to_string(), Value::from(n));
                }
            }
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
            if is_missing_table(&s) {
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
) -> Result<Vec<Value>, String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let masters_sql =
        er_golden::masters_cte_sql(policy, "lake.main", source_ranks).map_err(|e| e.to_string())?;
    let since_filter = since
        .filter(|s| !s.is_empty())
        .map(|s| format!("AND occurred_at >= {}::timestamp", lit(s)))
        .unwrap_or_default();
    // Protect enforcement (PR 1.3b): a member record with an active `protect`
    // overlay forces itself master for its entity, overriding the policy
    // election. Read-path only — no golden/fixture touched. Gated on the overlay
    // table existing AND holding at least one active protect row, so when no
    // records are protected (or the steward drain never created the table) the
    // CTE collapses to today's exact `masters AS ( {masters_sql} )` — byte-exact.
    let protect_active = match session.query_strings(
        "SELECT 1 FROM lake.main.record_overlays WHERE kind = 'protect' AND active LIMIT 1",
    ) {
        Ok(rows) => !rows.is_empty(),
        Err(e) => {
            let s = e.to_string();
            if is_missing_table(&s) {
                false
            } else {
                return Err(s);
            }
        }
    };
    let masters_cte = if protect_active {
        // `protected` picks, per affected entity, the active-protected member
        // (min(record_key) is a deterministic tie-break when several members of
        // one entity are protected). COALESCE lets it win over the policy master.
        format!(
            "masters_policy AS ( {masters_sql} ), \
             protected AS ( \
               SELECT m.entity_id, min(m.record_key) AS master_key \
               FROM lake.main.entity_membership m \
               JOIN affected a ON a.entity_id = m.entity_id \
               JOIN lake.main.record_overlays o \
                 ON o.record_key = m.record_key AND o.kind = 'protect' AND o.active \
               GROUP BY m.entity_id \
             ), \
             masters AS ( \
               SELECT p.entity_id, COALESCE(pr.master_key, p.master_key) AS master_key \
               FROM masters_policy p \
               LEFT JOIN protected pr ON pr.entity_id = p.entity_id \
             )"
        )
    } else {
        format!("masters AS ( {masters_sql} )")
    };
    let sql = format!(
        "WITH affected AS ( \
           SELECT DISTINCT entity_id FROM lake.main.entity_events \
           WHERE event_type IN ('merged', 'member_added') {since_filter} \
         ), \
         {masters_cte} \
         SELECT to_json(t) FROM ( \
           SELECT m.entity_id, ms.master_key, \
                  list(m.source_system || ':' || m.source_record_id ORDER BY m.record_key) \
                    AS member_records, \
                  count(*) AS member_count, \
                  any_value(g.given_name) AS given_name, \
                  any_value(g.family_name) AS family_name, \
                  any_value(g.email) AS email, \
                  any_value(g.phone_e164) AS phone_e164 \
           FROM lake.main.entity_membership m \
           JOIN masters ms ON ms.entity_id = m.entity_id \
           LEFT JOIN lake.main.golden_records g ON g.entity_id = m.entity_id \
           GROUP BY m.entity_id, ms.master_key HAVING count(*) >= 2 \
           ORDER BY m.entity_id LIMIT {limit} \
         ) t ORDER BY entity_id"
    );
    let rows = session.query_strings(&sql).map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| serde_json::from_str(&r[0]).unwrap_or(Value::Null))
        .collect())
}

/// Render merge-plan rows as CSV (`readapi.merge_plans` text/csv): the
/// `;`-joined member list, else the golden preview columns.
#[must_use]
pub fn merge_plans_csv(items: &[Value]) -> String {
    let mut out = String::from(
        "entity_id,master_key,member_count,member_records,golden_given_name,\
         golden_family_name,golden_email,golden_phone_e164\n",
    );
    let field = |v: &Value, k: &str| -> String {
        match v.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    };
    let csv_cell = |s: &str| {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    for item in items {
        let members = item
            .get("member_records")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(";")
            })
            .unwrap_or_default();
        let row = [
            field(item, "entity_id"),
            field(item, "master_key"),
            field(item, "member_count"),
            members,
            field(item, "given_name"),
            field(item, "family_name"),
            field(item, "email"),
            field(item, "phone_e164"),
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

/// Dashboard counters (`readapi.metrics`), snapshot-pinned: record/entity totals
/// and duplicate-group sizing, plus the open-review count (0 when the lake has
/// no `review_queue` yet — that table is written by the review stage, not the
/// base pipeline).
///
/// # Errors
/// Propagates a lake query error (other than a missing `review_queue`).
pub fn metrics(session: &Session, snapshot: i64) -> Result<Value, String> {
    let records = std_record_count(session, snapshot)?;
    let entities = golden_record_count(session, snapshot)?;
    let (duplicate_groups, records_in_dupes) = dup_group_stats(session, snapshot)?;
    // review_queue is optional in a base-pipeline lake; absence ⇒ 0 open.
    let open_reviews = er_entities::review::read::open_count(session.connection(), snapshot)
        .ok()
        .and_then(|rows| rows.first().and_then(|r| r.n))
        .unwrap_or(0);
    Ok(json!({
        "records": records,
        "entities": entities,
        "duplicate_groups": duplicate_groups,
        "records_in_duplicate_groups": records_in_dupes,
        "open_reviews": open_reviews,
        "snapshot": snapshot,
    }))
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
pub fn data_quality(session: &Session, snapshot: i64, stale_days: i64) -> Result<Value, String> {
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
    let mut by_attribute = serde_json::Map::new();
    let mut comp_sum = 0.0;
    for (name, count) in counts {
        let cnt = count.unwrap_or(0) as f64;
        let frac = if n_golden > 0.0 { cnt / n_golden } else { 0.0 };
        comp_sum += frac;
        by_attribute.insert(name.to_string(), json!(frac));
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

    Ok(json!({
        "dq_score": dq_score,
        "completeness": { "overall": completeness, "by_attribute": Value::Object(by_attribute) },
        "junk_rate": junk_rate,
        "stale_rate": stale_rate,
        "duplicate_rate": duplicate_rate,
        "records": records,
        "entities": entities,
        "stale_days": stale_days,
        "snapshot": snapshot,
    }))
}
