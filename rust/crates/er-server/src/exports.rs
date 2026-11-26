//! Bulk export of a dataset's results at one pinned snapshot (#1230, E11).
//!
//! An export IS a job: `POST …/exports` enqueues kind `export` with an
//! [`ExportRequest`] as its params, the `export` pod task writes ONE artifact
//! with [`write()`], and `GET …/exports/{id}` reads the job row back. There is no
//! export table, because the job row already holds the state, the params, the
//! error and the timestamps, and a second row would be a second truth.
//!
//! ## Where the artifact lives, and why it is not a presigned URL
//!
//! Beside the dataset's lake, under `<lake>/exports/`, never inside the lake's
//! data directory: DuckLake owns that directory, and its orphan-file cleanup
//! would delete a file its catalog does not list. The download streams through
//! er-server behind the org guard rather than handing out a presigned URL:
//! `duckfleet_artifacts.rs` states the house policy that signed URLs never
//! travel in a response.
//!
//! ## An S3-backed lake (#1240)
//!
//! The artifact goes to `s3://<bucket>/<tenant prefix>exports/<id>.<ext>`,
//! under the org's prefix in `tenant-runners.json`, never the data path's
//! parent: with a data path like `s3://b/prod/app/acme/` that parent is shared
//! by every tenant. So the lake's data path must sit strictly BELOW the prefix
//! (`<prefix>lake/`, say), or the artifact would land inside DuckLake's data
//! directory; [`artifact_location`] refuses either mistake. The runner writes it
//! with its own tenant identity and tags it `er-export=1`, which the lake
//! bucket's lifecycle rule expires. The API streams it back by assuming that
//! org's export READER, a role that can read only `<prefix>exports/*`, so a
//! cross-tenant read is refused by IAM and not only by the guard. An org with
//! no reader in [`readers`] is not enrolled, and its S3 export answers 501 at
//! enqueue rather than being accepted and then made undownloadable.
//!
//! ## What "complete" means
//!
//! Every page of one export reads the same snapshot, and the writer walks the
//! keyset until the cursor runs out, so the artifact is the whole result set at
//! that snapshot or the export fails. It is never a truncated page that looks
//! like a full dataset: past [`MAX_ROWS`] the export fails as a precondition
//! naming the ceiling. The file is written to a `.partial` name and renamed
//! into place, so a download can never see half of one.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use dedupe_core::session::Session;
use er_config::EntityType;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most rows one export may hold. A bound rather than a limit: an export
/// that would exceed it fails, naming it, instead of silently stopping short.
pub const MAX_ROWS: usize = 1_000_000;

/// How long a finished export stays downloadable.
pub const RETENTION: chrono::Duration = chrono::Duration::days(7);

/// What to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    /// One row per golden record: the dataset entity type's golden columns.
    Golden,
    /// One row per duplicate group: the entity, its members, confidence and
    /// golden preview, as the duplicates read serves it.
    Duplicates,
    /// One row per SOURCE record: `record_key`, `source_system`,
    /// `source_record_id` and the `entity_id` it resolved to (#1231). The join
    /// a customer makes from golden records and duplicate groups back to the
    /// keys in their own systems.
    Lineage,
}

/// The artifact's file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// Comma-separated, with a header row. Golden records only: a duplicate
    /// group's member list does not flatten into one row of cells.
    Csv,
    /// JSON Lines: one JSON object per line.
    Jsonl,
    /// Apache Parquet, Snappy-compressed: golden records and lineage, with
    /// the same columns their CSV carries (#1241). The format a warehouse
    /// load wants. Not duplicates: a group's nested shape has no schema
    /// fixed before its first page, so it stays JSON Lines.
    Parquet,
}

impl ExportFormat {
    /// Every format, for the gates that must cover each.
    pub const ALL: [ExportFormat; 3] = [
        ExportFormat::Csv,
        ExportFormat::Jsonl,
        ExportFormat::Parquet,
    ];

    /// The file extension.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Jsonl => "jsonl",
            ExportFormat::Parquet => "parquet",
        }
    }

    /// The `Content-Type` a download is served with.
    #[must_use]
    pub fn content_type(self) -> &'static str {
        match self {
            ExportFormat::Csv => "text/csv; charset=utf-8",
            ExportFormat::Jsonl => "application/x-ndjson",
            ExportFormat::Parquet => "application/vnd.apache.parquet",
        }
    }
}

/// An export request, as `POST …/exports` accepts it and as the job carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub kind: ExportKind,
    pub format: ExportFormat,
    /// The lake snapshot to export. Optional on the request (the latest is
    /// pinned at enqueue); always present on the job, so the pod task can
    /// never export a different snapshot than the one the API answered with.
    #[serde(default)]
    pub snapshot: Option<i64>,
    /// The org's S3 tenant prefix, set by the API at enqueue for an S3-backed
    /// lake and never accepted from a client (`create_export` builds the params
    /// from the three fields above). The runner writes under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_prefix: Option<String>,
}

impl ExportRequest {
    /// Parse and validate a request or a job's params.
    ///
    /// # Errors
    /// A message naming what is wrong: unparseable, or a combination this
    /// export does not produce.
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let request: Self = serde_json::from_value(params.clone())
            .map_err(|e| format!("invalid export request: {e}"))?;
        if request.kind == ExportKind::Duplicates && request.format != ExportFormat::Jsonl {
            return Err(
                "duplicates export as jsonl only: a group's member list does not flatten into CSV, \
                 and its nested shape has no Parquet schema fixed before the first page"
                    .to_string(),
            );
        }
        Ok(request)
    }
}

/// Where export `export_id`'s artifact lives for a dataset whose file-system
/// lake env is `lake_env`: `<lake>/exports/<export_id>.<ext>`, beside the
/// lake's data directory and never inside it.
///
/// # Errors
/// The env names no data path, or the lake is on S3 ([`artifact_location`]
/// handles that).
pub fn artifact_path(
    lake_env: &Value,
    export_id: &str,
    format: ExportFormat,
) -> Result<PathBuf, String> {
    let data = lake_env
        .get("ER_LAKE_DATA_PATH")
        .and_then(Value::as_str)
        .ok_or("the dataset's lake env names no ER_LAKE_DATA_PATH")?;
    if data.starts_with("s3://") {
        return Err(
            "an S3-backed lake has no file-system export path; see artifact_location".to_string(),
        );
    }
    // The id is a UUIDv7 job id; refuse anything else rather than build a
    // path from it.
    if !valid_export_id(export_id) {
        return Err(format!("invalid export id '{export_id}'"));
    }
    let lake = Path::new(data.trim_end_matches('/'))
        .parent()
        .ok_or("the lake data path has no parent directory")?;
    Ok(lake
        .join("exports")
        .join(format!("{export_id}.{}", format.extension())))
}

/// Where an export's artifact lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactLocation {
    /// A file beside an EFS or local lake.
    File(PathBuf),
    /// An object under the org's tenant prefix in the lake bucket.
    S3 { bucket: String, key: String },
}

/// Where export `export_id` lives for a lake env: [`artifact_path`] for a
/// file-system lake, `s3://<bucket>/<tenant_prefix>exports/<id>.<ext>` for an
/// S3 one.
///
/// # Errors
/// An S3 lake with no tenant prefix (the org is not enrolled), a data path not
/// strictly below that prefix, or anything [`artifact_path`] refuses.
pub fn artifact_location(
    lake_env: &Value,
    export_id: &str,
    format: ExportFormat,
    tenant_prefix: Option<&str>,
) -> Result<ArtifactLocation, String> {
    let data = lake_env
        .get("ER_LAKE_DATA_PATH")
        .and_then(Value::as_str)
        .ok_or("the dataset's lake env names no ER_LAKE_DATA_PATH")?;
    let Some(rest) = data.strip_prefix("s3://") else {
        return artifact_path(lake_env, export_id, format).map(ArtifactLocation::File);
    };
    let prefix = tenant_prefix.ok_or(
        "this org's lake is on S3 but it has no export reader: enroll it in tenant-runners.json",
    )?;
    if !valid_export_id(export_id) {
        return Err(format!("invalid export id '{export_id}'"));
    }
    let (bucket, path) = rest.split_once('/').unwrap_or((rest, ""));
    if bucket.is_empty() || prefix.is_empty() || !prefix.ends_with('/') || prefix.contains("..") {
        return Err(format!("invalid S3 export location s3://{bucket}/{prefix}"));
    }
    // Strictly below the prefix: at the prefix itself, `<prefix>exports/`
    // would sit inside DuckLake's data directory, whose orphan cleanup
    // deletes what its catalog does not list.
    let below = path
        .strip_prefix(prefix)
        .is_some_and(|tail| !tail.trim_matches('/').is_empty() && !tail.starts_with("exports/"));
    if !below {
        return Err(format!(
            "the lake's data path {data} must sit strictly below its tenant prefix {prefix} \
             (and not at {prefix}exports/) for exports to live beside it"
        ));
    }
    Ok(ArtifactLocation::S3 {
        bucket: bucket.to_string(),
        key: format!("{prefix}exports/{export_id}.{}", format.extension()),
    })
}

fn valid_export_id(export_id: &str) -> bool {
    !export_id.is_empty()
        && export_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// An enrolled org's export reader (#1240): the role the API assumes to read
/// the org's artifacts, and the org's tenant prefix.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportReader {
    pub role: String,
    pub prefix: String,
}

/// Parse `ER_EXPORT_READ_ROLES`, the chart's `{org: {role, prefix}}` map.
/// Absent or empty is no readers.
///
/// # Errors
/// Not that JSON shape.
pub fn parse_readers(raw: Option<&str>) -> Result<BTreeMap<String, ExportReader>, String> {
    match raw.map(str::trim) {
        None | Some("") => Ok(BTreeMap::new()),
        Some(raw) => serde_json::from_str(raw)
            .map_err(|e| format!("ER_EXPORT_READ_ROLES must map org ids to {{role, prefix}}: {e}")),
    }
}

/// This process's export readers, read once from `ER_EXPORT_READ_ROLES`. A
/// malformed value is logged and treated as none, which refuses S3 exports
/// rather than guessing at a role.
pub fn readers() -> &'static BTreeMap<String, ExportReader> {
    static READERS: OnceLock<BTreeMap<String, ExportReader>> = OnceLock::new();
    READERS.get_or_init(|| {
        parse_readers(std::env::var("ER_EXPORT_READ_ROLES").ok().as_deref()).unwrap_or_else(|e| {
            tracing::error!(error = %e, "invalid export reader configuration; S3 exports are off");
            BTreeMap::new()
        })
    })
}

/// When an export finished at `finished_at` stops being downloadable.
#[must_use]
pub fn expires_at(finished_at: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    finished_at + RETENTION
}

/// Delete the dataset's expired artifacts: finished files older than
/// [`RETENTION`], and `.partial` files older than a day (an export that died
/// mid-write). Returns how many were removed. Called by `lake_maintain`; the
/// download refuses an expired export by its job's `finished_at` regardless,
/// so the sweep reclaims space rather than enforcing anything.
///
/// A missing exports directory is nothing to sweep, and an S3-backed lake has
/// no local artifacts.
///
/// # Errors
/// A directory that exists but cannot be read, or a file that cannot be removed.
pub fn sweep(lake_env: &Value, now: std::time::SystemTime) -> Result<usize, String> {
    let Ok(probe) = artifact_path(lake_env, "sweep", ExportFormat::Csv) else {
        return Ok(0);
    };
    let Some(dir) = probe.parent() else {
        return Ok(0);
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("exports dir: {e}")),
    };
    let retention = RETENTION.to_std().unwrap_or_default();
    let partial_limit = std::time::Duration::from_hours(24);
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|e| format!("exports dir: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let limit = if name.ends_with(".partial") {
            partial_limit
        } else {
            retention
        };
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .map_err(|e| format!("export stat {name}: {e}"))?;
        if now.duration_since(modified).unwrap_or_default() > limit {
            std::fs::remove_file(entry.path()).map_err(|e| format!("export sweep {name}: {e}"))?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// What [`write()`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    pub rows: usize,
    pub bytes: u64,
}

/// Write `request`'s artifact to `path`, walking every page at its snapshot.
/// `page` is the page size and `max_rows` the ceiling ([`MAX_ROWS`] in
/// production); both are parameters so a test can force several pages and hit
/// the ceiling without a million rows.
///
/// # Errors
/// A lake read failure, the [`MAX_ROWS`] ceiling, or an I/O failure. On any
/// error nothing is left at `path`.
pub fn write(
    session: &Session,
    entity_type: EntityType,
    request: &ExportRequest,
    path: &Path,
    page: i64,
    max_rows: usize,
) -> Result<ExportReport, String> {
    let snapshot = request
        .snapshot
        .ok_or("an export job must carry the snapshot it was pinned to")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("export dir: {e}"))?;
    }
    let partial = path.with_extension(format!(
        "{}.partial",
        path.extension().and_then(|e| e.to_str()).unwrap_or("out")
    ));
    let result = write_to(
        session,
        entity_type,
        request,
        snapshot,
        &partial,
        page,
        max_rows,
    );
    match result {
        Ok(rows) => {
            std::fs::rename(&partial, path).map_err(|e| format!("export publish: {e}"))?;
            let bytes = std::fs::metadata(path)
                .map_err(|e| format!("export stat: {e}"))?
                .len();
            Ok(ExportReport { rows, bytes })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

fn write_to(
    session: &Session,
    entity_type: EntityType,
    request: &ExportRequest,
    snapshot: i64,
    partial: &Path,
    page: i64,
    max_rows: usize,
) -> Result<usize, String> {
    let file = std::fs::File::create(partial).map_err(|e| format!("export file: {e}"))?;
    let mut out = std::io::BufWriter::new(file);
    let mut rows = 0usize;
    let ceiling = |rows: usize| {
        if rows > max_rows {
            Err(format!(
                "the export exceeds the {max_rows}-row ceiling; narrow it, or ask for a higher bound"
            ))
        } else {
            Ok(())
        }
    };
    match request.kind {
        ExportKind::Golden => match request.format {
            ExportFormat::Csv => {
                let mut writer = arrow::csv::Writer::new(&mut out);
                golden_pages(session, entity_type, snapshot, page, |batch| {
                    rows += batch.num_rows();
                    ceiling(rows)?;
                    writer.write(batch).map_err(|e| format!("export csv: {e}"))
                })?;
            }
            ExportFormat::Jsonl => {
                golden_pages(session, entity_type, snapshot, page, |batch| {
                    rows += batch.num_rows();
                    ceiling(rows)?;
                    let mut writer = arrow::json::LineDelimitedWriter::new(&mut out);
                    writer
                        .write(batch)
                        .map_err(|e| format!("export jsonl: {e}"))?;
                    writer.finish().map_err(|e| format!("export jsonl: {e}"))
                })?;
            }
            ExportFormat::Parquet => {
                // The writer needs a schema before its first batch, so it is
                // opened on the first page; an empty export takes the entity's
                // declared schema, and is still a readable file.
                let mut sink = Some(&mut out);
                let mut writer = None;
                golden_pages(session, entity_type, snapshot, page, |batch| {
                    rows += batch.num_rows();
                    ceiling(rows)?;
                    if writer.is_none() {
                        let out = sink.take().ok_or("export parquet: no output")?;
                        writer = Some(parquet_writer(out, batch.schema())?);
                    }
                    writer
                        .as_mut()
                        .ok_or("export parquet: no writer")?
                        .write(batch)
                        .map_err(|e| format!("export parquet: {e}"))
                })?;
                let writer = match writer {
                    Some(writer) => writer,
                    None => parquet_writer(
                        sink.take().ok_or("export parquet: no output")?,
                        std::sync::Arc::new(match entity_type {
                            EntityType::Person => {
                                er_entities::golden_record::GoldenRecord::arrow_schema()
                            }
                            EntityType::Account => {
                                er_entities::acct_golden_record::AcctGoldenRecord::arrow_schema()
                            }
                        }),
                    )?,
                };
                writer.close().map_err(|e| format!("export parquet: {e}"))?;
            }
        },
        ExportKind::Lineage if request.format == ExportFormat::Parquet => {
            let schema = std::sync::Arc::new(arrow::datatypes::Schema::new(
                LINEAGE_COLUMNS
                    .map(|name| {
                        arrow::datatypes::Field::new(name, arrow::datatypes::DataType::Utf8, false)
                    })
                    .to_vec(),
            ));
            let mut writer = parquet_writer(&mut out, schema.clone())?;
            let mut after: Option<String> = None;
            loop {
                let page_rows = match er_entities::entity_membership::read::lineage_page(
                    session.connection(),
                    snapshot,
                    after.as_deref(),
                    page,
                ) {
                    Ok(found) => found,
                    Err(e) if after.is_none() && not_yet_written(&e.to_string()) => break,
                    Err(e) => return Err(e.to_string()),
                };
                rows += page_rows.len();
                ceiling(rows)?;
                if !page_rows.is_empty() {
                    let column = |get: fn(&_) -> &str| -> arrow::array::ArrayRef {
                        std::sync::Arc::new(arrow::array::StringArray::from_iter_values(
                            page_rows.iter().map(get),
                        ))
                    };
                    let batch = arrow::record_batch::RecordBatch::try_new(
                        schema.clone(),
                        vec![
                            column(|r: &er_entities::entity_membership::read::LineagePageRow| {
                                r.record_key.as_str()
                            }),
                            column(|r: &er_entities::entity_membership::read::LineagePageRow| {
                                r.source_system.as_str()
                            }),
                            column(|r: &er_entities::entity_membership::read::LineagePageRow| {
                                r.source_record_id.as_str()
                            }),
                            column(|r: &er_entities::entity_membership::read::LineagePageRow| {
                                r.entity_id.as_str()
                            }),
                        ],
                    )
                    .map_err(|e| format!("export parquet: {e}"))?;
                    writer
                        .write(&batch)
                        .map_err(|e| format!("export parquet: {e}"))?;
                }
                match (page_rows.len() as i64 == page, page_rows.last()) {
                    (true, Some(last)) => after = Some(last.record_key.clone()),
                    _ => break,
                }
            }
            writer.close().map_err(|e| format!("export parquet: {e}"))?;
        }
        ExportKind::Lineage => {
            if request.format == ExportFormat::Csv {
                out.write_all(format!("{}\n", LINEAGE_COLUMNS.join(",")).as_bytes())
                    .map_err(|e| format!("export write: {e}"))?;
            }
            let mut after: Option<String> = None;
            loop {
                let page_rows = match er_entities::entity_membership::read::lineage_page(
                    session.connection(),
                    snapshot,
                    after.as_deref(),
                    page,
                ) {
                    Ok(found) => found,
                    Err(e) if after.is_none() && not_yet_written(&e.to_string()) => break,
                    Err(e) => return Err(e.to_string()),
                };
                let full = page_rows.len() as i64 == page;
                for row in &page_rows {
                    rows += 1;
                    ceiling(rows)?;
                    match request.format {
                        ExportFormat::Csv => {
                            let line = [
                                row.record_key.as_str(),
                                row.source_system.as_str(),
                                row.source_record_id.as_str(),
                                row.entity_id.as_str(),
                            ]
                            .map(csv_field)
                            .join(",");
                            out.write_all(line.as_bytes())
                                .and_then(|()| out.write_all(b"\n"))
                                .map_err(|e| format!("export write: {e}"))?;
                        }
                        ExportFormat::Jsonl | ExportFormat::Parquet => {
                            serde_json::to_writer(
                                &mut out,
                                &serde_json::json!({
                                    "record_key": row.record_key,
                                    "source_system": row.source_system,
                                    "source_record_id": row.source_record_id,
                                    "entity_id": row.entity_id,
                                }),
                            )
                            .map_err(|e| format!("export jsonl: {e}"))?;
                            out.write_all(b"\n")
                                .map_err(|e| format!("export write: {e}"))?;
                        }
                    }
                }
                match (full, page_rows.last()) {
                    (true, Some(last)) => after = Some(last.record_key.clone()),
                    _ => break,
                }
            }
        }
        ExportKind::Duplicates => {
            let mut after: Option<String> = None;
            loop {
                let (groups, next) = match crate::read::duplicates(
                    session,
                    entity_type,
                    snapshot,
                    after.as_deref(),
                    &crate::read::DuplicateFilters::default(),
                    page,
                ) {
                    Ok(found) => found,
                    Err(e) if after.is_none() && not_yet_written(&e) => break,
                    Err(e) => return Err(e),
                };
                for group in groups.to_values() {
                    rows += 1;
                    ceiling(rows)?;
                    serde_json::to_writer(&mut out, &group)
                        .map_err(|e| format!("export jsonl: {e}"))?;
                    out.write_all(b"\n")
                        .map_err(|e| format!("export write: {e}"))?;
                }
                match next {
                    Some(cursor) => {
                        after = Some(
                            crate::read::decode_cursor(&cursor)
                                .ok_or("the duplicates page returned an unreadable cursor")?
                                .key,
                        );
                    }
                    None => break,
                }
            }
        }
    }
    out.flush().map_err(|e| format!("export flush: {e}"))?;
    Ok(rows)
}

/// One CSV field, quoted when it holds a comma, a quote or a line break
/// (RFC 4180): a customer's source id is arbitrary text.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Whether a first-page read failed because the relation it reads does not
/// exist at the export's snapshot: a lake no run has finished in. That is an
/// empty result, a zero-row artifact, not a failure, so "nothing yet" and
/// "the export broke" stay different answers.
fn not_yet_written(error: &str) -> bool {
    error.contains("Catalog Error: Table with name ") && error.contains(" does not exist")
}

/// Walk every golden page at `snapshot`, handing each batch to `each`.
/// The lineage export's columns, in order, for every format.
const LINEAGE_COLUMNS: [&str; 4] = [
    "record_key",
    "source_system",
    "source_record_id",
    "entity_id",
];

/// A Snappy-compressed Parquet writer over `out` for `schema`.
fn parquet_writer<W: std::io::Write + Send>(
    out: W,
    schema: arrow::datatypes::SchemaRef,
) -> Result<parquet::arrow::ArrowWriter<W>, String> {
    let props = parquet::file::properties::WriterProperties::builder()
        .set_compression(parquet::basic::Compression::SNAPPY)
        .build();
    parquet::arrow::ArrowWriter::try_new(out, schema, Some(props))
        .map_err(|e| format!("export parquet: {e}"))
}

fn golden_pages(
    session: &Session,
    entity_type: EntityType,
    snapshot: i64,
    page: i64,
    mut each: impl FnMut(&arrow::record_batch::RecordBatch) -> Result<(), String>,
) -> Result<(), String> {
    let mut after: Option<String> = None;
    loop {
        let chunk = match crate::read::golden_arrow_page(
            session,
            entity_type,
            snapshot,
            None,
            after.as_deref(),
            page,
        ) {
            Ok(chunk) => chunk,
            Err(e) if after.is_none() && not_yet_written(&e) => return Ok(()),
            Err(e) => return Err(e),
        };
        for batch in &chunk.batches {
            each(batch)?;
        }
        match chunk.next_cursor {
            Some(cursor) => {
                after = Some(
                    crate::read::decode_cursor(&cursor)
                        .ok_or("the golden page returned an unreadable cursor")?
                        .key,
                );
            }
            None => return Ok(()),
        }
    }
}

/// The S3 half of an export (#1240): the runner's tagged upload and the API's
/// read through the org's export reader.
pub mod s3 {
    use aws_sdk_s3::primitives::ByteStream;

    /// The tag the lake bucket's `export-expiry` lifecycle rule matches.
    pub const EXPIRY_TAG: &str = "er-export=1";

    fn https() -> aws_sdk_s3::config::SharedHttpClient {
        // Ring-backed rustls, for the reason mail.rs gives: the SDK's default
        // TLS needs a cmake the build image lacks.
        aws_smithy_http_client::Builder::new()
            .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
                aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
            ))
            .build_https()
    }

    /// Upload the finished file at `from` to `s3://bucket/key`, tagged for
    /// expiry, with the process's own credentials (the runner's tenant
    /// identity). Blocking: the export task is synchronous, so this builds a
    /// current-thread runtime of its own. A single PUT is atomic, so a
    /// download never sees half an artifact.
    ///
    /// # Errors
    /// The runtime, the file or the PUT failed.
    pub fn put_blocking(
        bucket: &str,
        key: &str,
        from: &std::path::Path,
        content_type: &str,
    ) -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("export upload runtime: {e}"))?;
        runtime.block_on(async {
            let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .http_client(https())
                .load()
                .await;
            let body = ByteStream::from_path(from)
                .await
                .map_err(|e| format!("export upload body: {e}"))?;
            aws_sdk_s3::Client::new(&config)
                .put_object()
                .bucket(bucket)
                .key(key)
                .content_type(content_type)
                .tagging(EXPIRY_TAG)
                .body(body)
                .send()
                .await
                .map(|_| ())
                .map_err(|e| {
                    format!(
                        "export upload s3://{bucket}/{key}: {}",
                        aws_sdk_s3::error::DisplayErrorContext(&e)
                    )
                })
        })
    }

    /// Why a read failed.
    #[derive(Debug)]
    pub enum ReadError {
        /// No such object: the lifecycle rule reached it first.
        Missing,
        /// Anything else, AccessDenied included.
        Other(String),
    }

    /// Open `s3://bucket/key` as the org's export reader `role`.
    ///
    /// # Errors
    /// [`ReadError::Missing`] for an absent object, otherwise the SDK's error.
    pub async fn open(role: &str, bucket: &str, key: &str) -> Result<ByteStream, ReadError> {
        let base = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .http_client(https())
            .load()
            .await;
        let reader = aws_config::sts::AssumeRoleProvider::builder(role)
            .session_name("er-api-export-download")
            .configure(&base)
            .build()
            .await;
        let config = aws_sdk_s3::config::Builder::from(&base)
            .credentials_provider(reader)
            .build();
        match aws_sdk_s3::Client::from_conf(config)
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
        {
            Ok(object) => Ok(object.body),
            Err(e)
                if e.as_service_error().is_some_and(
                    aws_sdk_s3::operation::get_object::GetObjectError::is_no_such_key,
                ) =>
            {
                Err(ReadError::Missing)
            }
            Err(e) => Err(ReadError::Other(format!(
                "export read s3://{bucket}/{key}: {}",
                aws_sdk_s3::error::DisplayErrorContext(&e)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_parses_and_refuses_duplicates_as_csv() {
        assert_eq!(
            ExportRequest::from_params(&json!({"kind": "golden", "format": "csv"})),
            Ok(ExportRequest {
                kind: ExportKind::Golden,
                format: ExportFormat::Csv,
                snapshot: None,
                tenant_prefix: None,
            })
        );
        assert!(
            ExportRequest::from_params(&json!({"kind": "duplicates", "format": "csv"}))
                .unwrap_err()
                .contains("jsonl only")
        );
        assert!(ExportRequest::from_params(&json!({"kind": "golden", "format": "xlsx"})).is_err());
        assert!(
            ExportRequest::from_params(&json!({"kind": "golden", "format": "csv", "extra": 1}))
                .is_err(),
            "unknown fields are refused, not ignored"
        );
    }

    #[test]
    fn the_artifact_sits_beside_the_lake_data_never_in_it() {
        let env = json!({"ER_LAKE_DATA_PATH": "/efs/acme/default/lake/data/"});
        assert_eq!(
            artifact_path(&env, "0193-abc", ExportFormat::Csv).unwrap(),
            PathBuf::from("/efs/acme/default/lake/exports/0193-abc.csv")
        );
    }

    #[test]
    fn a_csv_field_is_quoted_only_when_it_must_be() {
        assert_eq!(csv_field("crm"), "crm");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
    }

    #[test]
    fn the_sweep_removes_only_what_has_expired() {
        let root = std::env::temp_dir().join(format!("er_export_sweep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("lake/exports");
        std::fs::create_dir_all(&dir).unwrap();
        let env = json!({"ER_LAKE_DATA_PATH": root.join("lake/data").to_string_lossy()});
        let now = std::time::SystemTime::now();
        let aged = |name: &str, age: std::time::Duration| {
            let f = std::fs::File::create(dir.join(name)).unwrap();
            f.set_modified(now - age).unwrap();
        };
        let day = std::time::Duration::from_hours(24);
        aged("fresh.csv", day);
        aged("old.csv", day * 8);
        aged("fresh.jsonl.partial", std::time::Duration::from_hours(2));
        aged("dead.jsonl.partial", day * 2);
        assert_eq!(sweep(&env, now), Ok(2));
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["fresh.csv", "fresh.jsonl.partial"]);
        // Nothing to sweep is not an error.
        assert_eq!(
            sweep(&json!({"ER_LAKE_DATA_PATH": "/nonexistent/lake/data"}), now),
            Ok(0)
        );
        assert_eq!(
            sweep(&json!({"ER_LAKE_DATA_PATH": "s3://b/p/"}), now),
            Ok(0)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_s3_export_lives_under_the_tenant_prefix_beside_the_lake() {
        let s3 = json!({"ER_LAKE_DATA_PATH": "s3://bucket/_isolation/probe-a/lake/"});
        assert_eq!(
            artifact_location(
                &s3,
                "0193-abc",
                ExportFormat::Csv,
                Some("_isolation/probe-a/")
            ),
            Ok(ArtifactLocation::S3 {
                bucket: "bucket".into(),
                key: "_isolation/probe-a/exports/0193-abc.csv".into(),
            })
        );
        // A file-system lake ignores the prefix and keeps its path.
        let local = json!({"ER_LAKE_DATA_PATH": "/efs/acme/lake/data"});
        assert_eq!(
            artifact_location(&local, "0193-abc", ExportFormat::Jsonl, None),
            Ok(ArtifactLocation::File(PathBuf::from(
                "/efs/acme/lake/exports/0193-abc.jsonl"
            )))
        );
        // The S3 branch has no file-system path.
        assert!(artifact_path(&s3, "0193-abc", ExportFormat::Csv).is_err());
    }

    #[test]
    fn an_s3_export_outside_its_tenant_or_inside_the_lake_is_refused() {
        let at = |data: &str, prefix: Option<&str>, id: &str| {
            artifact_location(
                &json!({"ER_LAKE_DATA_PATH": data}),
                id,
                ExportFormat::Csv,
                prefix,
            )
        };
        // Not enrolled: no reader, no prefix.
        assert!(
            at("s3://b/a/lake/", None, "x")
                .unwrap_err()
                .contains("enroll")
        );
        // The data path IS the prefix: exports/ would sit inside DuckLake's
        // data directory and orphan cleanup would delete it.
        assert!(at("s3://b/a/", Some("a/"), "x").is_err());
        // A data path under ANOTHER tenant's prefix.
        assert!(at("s3://b/other/lake/", Some("a/"), "x").is_err());
        // A data path that is the exports dir itself.
        assert!(at("s3://b/a/exports/", Some("a/"), "x").is_err());
        // Malformed prefixes and ids.
        assert!(at("s3://b/a/lake/", Some("a"), "x").is_err());
        assert!(at("s3://b/a/../lake/", Some("a/../"), "x").is_err());
        assert!(at("s3://b/a/lake/", Some("a/"), "../x").is_err());
        assert!(at("s3:///a/lake/", Some("a/"), "x").is_err());
    }

    #[test]
    fn export_readers_parse_from_the_chart_map() {
        assert_eq!(parse_readers(None), Ok(BTreeMap::new()));
        assert_eq!(parse_readers(Some(" ")), Ok(BTreeMap::new()));
        let readers = parse_readers(Some(
            r#"{"org-a":{"prefix":"a/","role":"arn:aws:iam::1:role/er-dev-export-x"}}"#,
        ))
        .unwrap();
        assert_eq!(
            readers["org-a"],
            ExportReader {
                role: "arn:aws:iam::1:role/er-dev-export-x".into(),
                prefix: "a/".into(),
            }
        );
        assert!(parse_readers(Some(r#"{"org-a":"arn"}"#)).is_err());
        assert!(parse_readers(Some(r#"{"org-a":{"role":"r","prefix":"a/","x":1}}"#)).is_err());
    }

    #[test]
    fn a_client_cannot_be_given_a_tenant_prefix_it_did_not_have() {
        // The job carries the prefix; a request without one parses to None,
        // and create_export never copies it from the body.
        let request =
            ExportRequest::from_params(&json!({"kind": "golden", "format": "csv"})).unwrap();
        assert_eq!(request.tenant_prefix, None);
        assert!(
            !serde_json::to_value(&request)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("tenant_prefix")
        );
    }

    #[test]
    fn a_path_shaped_id_is_refused() {
        let local = json!({"ER_LAKE_DATA_PATH": "/efs/acme/lake/data"});
        for id in ["../etc/passwd", "a/b", "", "x.y"] {
            assert!(
                artifact_path(&local, id, ExportFormat::Jsonl).is_err(),
                "{id:?}"
            );
        }
    }
}
