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
use serde_json::{Value, json};

/// The golden-records list page size when the caller gives none, and the cap
/// (`readapi.py` `limit <= 200`).
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

/// The golden-record attribute columns, in display order. `assembled_at` is
/// intentionally absent — er-golden omits that volatile column from the lake.
const GOLDEN_COLUMNS: &str = "entity_id, given_name, family_name, email, phone_e164, \
     addr_number, addr_street, addr_unit, addr_city, addr_region, addr_postal, \
     birth_date, metadata, survivorship_version";

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

/// A LIKE pattern literal: escape the LIKE metacharacters, then quote. Paired
/// with `ESCAPE '\'` at the use site.
fn like_contains(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    lit(&format!("%{escaped}%"))
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
pub fn golden_list(
    session: &Session,
    snapshot: i64,
    q: Option<&str>,
    after: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let mut filters = Vec::new();
    if let Some(after) = after {
        filters.push(format!("entity_id > {}", lit(after)));
    }
    if let Some(q) = q.filter(|s| !s.is_empty()) {
        filters.push(format!(
            "(coalesce(given_name,'') || ' ' || coalesce(family_name,'') || ' ' || \
             coalesce(email,'')) ILIKE {} ESCAPE '\\'",
            like_contains(q)
        ));
    }
    let where_sql = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };
    // Fetch one extra row to decide whether a next page exists.
    let sql = format!(
        "SELECT entity_id, to_json(t) FROM ( \
           SELECT {GOLDEN_COLUMNS} FROM lake.main.golden_records AT (VERSION => {snapshot}) \
           {where_sql} ORDER BY entity_id LIMIT {} \
         ) t ORDER BY entity_id",
        limit + 1
    );
    let rows = session.query_strings(&sql).map_err(|e| e.to_string())?;
    let has_more = rows.len() as i64 > limit;
    let page = &rows[..rows.len().min(limit as usize)];
    let items: Vec<Value> = page
        .iter()
        .map(|r| serde_json::from_str(&r[1]).unwrap_or(Value::Null))
        .collect();
    let next_cursor = if has_more {
        page.last().map(|r| encode_cursor(snapshot, &r[0]))
    } else {
        None
    };
    Ok((items, next_cursor))
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

/// Each row of an inner SELECT as a JSON object (NULLs/types preserved via
/// `to_json`).
fn json_rows(session: &Session, inner_sql: &str) -> Result<Vec<Value>, String> {
    let rows = session
        .query_strings(&format!("SELECT to_json(t) FROM ({inner_sql}) t"))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| serde_json::from_str(&r[0]).unwrap_or(Value::Null))
        .collect())
}

/// One resolved entity with its members, lineage, and events
/// (`readapi.entity_detail`), snapshot-pinned. `None` if the entity is absent
/// at that snapshot (→ 404).
///
/// # Errors
/// Propagates a lake query error.
pub fn entity_detail(
    session: &Session,
    snapshot: i64,
    entity_id: &str,
) -> Result<Option<Value>, String> {
    let eid = lit(entity_id);
    let at = format!("AT (VERSION => {snapshot})");
    let golden = json_rows(
        session,
        &format!(
            "SELECT {GOLDEN_COLUMNS} FROM lake.main.golden_records {at} WHERE entity_id = {eid}"
        ),
    )?;
    let Some(golden_row) = golden.into_iter().next() else {
        return Ok(None);
    };
    let members = json_rows(
        session,
        &format!(
            "SELECT source_system, source_record_id, record_key, assigned_at, run_id \
             FROM lake.main.entity_membership {at} WHERE entity_id = {eid} ORDER BY record_key"
        ),
    )?;
    let lineage = json_rows(
        session,
        &format!(
            "SELECT attribute, record_key, source_system, source_record_id, rule \
             FROM lake.main.golden_lineage {at} WHERE entity_id = {eid} \
             ORDER BY attribute, record_key"
        ),
    )?;
    let events = json_rows(
        session,
        &format!(
            "SELECT event_id, seq, run_id, event_type, details, occurred_at \
             FROM lake.main.entity_events {at} WHERE entity_id = {eid} ORDER BY occurred_at, seq"
        ),
    )?;
    Ok(Some(json!({
        "golden": golden_row,
        "members": members,
        "lineage": lineage,
        "events": events,
    })))
}

/// A snapshot-pinned scalar count.
fn count_at(session: &Session, sql: &str) -> Result<i64, String> {
    let rows = session.query_strings(sql).map_err(|e| e.to_string())?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0))
}

/// Dashboard counters (`readapi.metrics`), snapshot-pinned: record/entity totals
/// and duplicate-group sizing, plus the open-review count (0 when the lake has
/// no `review_queue` yet — that table is written by the review stage, not the
/// base pipeline).
///
/// # Errors
/// Propagates a lake query error (other than a missing `review_queue`).
pub fn metrics(session: &Session, snapshot: i64) -> Result<Value, String> {
    let at = format!("AT (VERSION => {snapshot})");
    let records = count_at(
        session,
        &format!("SELECT count(*) FROM lake.main.int_std_records {at}"),
    )?;
    let entities = count_at(
        session,
        &format!("SELECT count(*) FROM lake.main.golden_records {at}"),
    )?;
    let dup = session
        .query_strings(&format!(
            "SELECT count(*), coalesce(sum(c), 0) FROM ( \
               SELECT entity_id, count(*) AS c FROM lake.main.entity_membership {at} \
               GROUP BY entity_id HAVING count(*) >= 2 \
             ) g"
        ))
        .map_err(|e| e.to_string())?;
    let duplicate_groups: i64 = dup
        .first()
        .and_then(|r| r.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let records_in_dupes: i64 = dup
        .first()
        .and_then(|r| r.get(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // review_queue is optional in a base-pipeline lake; absence ⇒ 0 open.
    let open_reviews = session
        .query_strings(&format!(
            "SELECT count(*) FROM lake.main.review_queue {at} WHERE status = 'open'"
        ))
        .ok()
        .and_then(|r| {
            r.first()
                .and_then(|c| c.first())
                .and_then(|s| s.parse::<i64>().ok())
        })
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
