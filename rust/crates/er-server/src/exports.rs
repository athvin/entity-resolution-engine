//! Bulk export of a dataset's results at one pinned snapshot (#1230, E11).
//!
//! An export IS a job: `POST …/exports` enqueues kind `export` with an
//! [`ExportRequest`] as its params, the `export` pod task writes ONE artifact
//! with [`write`], and `GET …/exports/{id}` reads the job row back. There is no
//! export table, because the job row already holds the state, the params, the
//! error and the timestamps, and a second row would be a second truth.
//!
//! ## Where the artifact lives, and why it is not a presigned URL
//!
//! Beside the dataset's lake, under `<lake>/exports/`, never inside the lake's
//! data directory: DuckLake owns that directory, and its orphan-file cleanup
//! would delete a file its catalog does not list. The download streams through
//! er-server behind the org guard rather than handing out a presigned URL: the
//! API pod has no S3 grant at all, a presigning grant would have to span every
//! tenant's prefix, and `duckfleet_artifacts.rs` already states the house
//! policy that signed URLs never travel in a response. An S3-backed lake is
//! therefore refused at enqueue ([`artifact_path`]) rather than accepted and
//! then made undownloadable.
//!
//! ## What "complete" means
//!
//! Every page of one export reads the same snapshot, and the writer walks the
//! keyset until the cursor runs out, so the artifact is the whole result set at
//! that snapshot or the export fails. It is never a truncated page that looks
//! like a full dataset: past [`MAX_ROWS`] the export fails as a precondition
//! naming the ceiling. The file is written to a `.partial` name and renamed
//! into place, so a download can never see half of one.

use std::io::Write as _;
use std::path::{Path, PathBuf};

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
}

impl ExportFormat {
    /// The file extension.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Jsonl => "jsonl",
        }
    }

    /// The `Content-Type` a download is served with.
    #[must_use]
    pub fn content_type(self) -> &'static str {
        match self {
            ExportFormat::Csv => "text/csv; charset=utf-8",
            ExportFormat::Jsonl => "application/x-ndjson",
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
        if request.kind == ExportKind::Duplicates && request.format == ExportFormat::Csv {
            return Err(
                "duplicates export as jsonl only: a group's member list does not flatten into CSV"
                    .to_string(),
            );
        }
        Ok(request)
    }
}

/// Where export `export_id`'s artifact lives for a dataset whose lake env is
/// `lake_env`: `<lake>/exports/<export_id>.<ext>`, beside the lake's data
/// directory and never inside it.
///
/// # Errors
/// The env names no data path, or the lake is on S3, which the API pod cannot
/// read back for a download (see the module doc).
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
            "exports are not yet available for an S3-backed lake: the API cannot read the \
             artifact back to serve it"
                .to_string(),
        );
    }
    // The id is a UUIDv7 job id; refuse anything else rather than build a
    // path from it.
    if export_id.is_empty()
        || !export_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(format!("invalid export id '{export_id}'"));
    }
    let lake = Path::new(data.trim_end_matches('/'))
        .parent()
        .ok_or("the lake data path has no parent directory")?;
    Ok(lake
        .join("exports")
        .join(format!("{export_id}.{}", format.extension())))
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

/// What [`write`] produced.
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
        },
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

/// Whether a first-page read failed because the relation it reads does not
/// exist at the export's snapshot: a lake no run has finished in. That is an
/// empty result, a zero-row artifact, not a failure, so "nothing yet" and
/// "the export broke" stay different answers.
fn not_yet_written(error: &str) -> bool {
    error.contains("Catalog Error: Table with name ") && error.contains(" does not exist")
}

/// Walk every golden page at `snapshot`, handing each batch to `each`.
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
    fn an_s3_lake_and_a_path_shaped_id_are_refused() {
        let s3 = json!({"ER_LAKE_DATA_PATH": "s3://bucket/prod/app/acme/"});
        assert!(
            artifact_path(&s3, "0193-abc", ExportFormat::Csv)
                .unwrap_err()
                .contains("S3-backed")
        );
        let local = json!({"ER_LAKE_DATA_PATH": "/efs/acme/lake/data"});
        for id in ["../etc/passwd", "a/b", "", "x.y"] {
            assert!(
                artifact_path(&local, id, ExportFormat::Jsonl).is_err(),
                "{id:?}"
            );
        }
    }
}
