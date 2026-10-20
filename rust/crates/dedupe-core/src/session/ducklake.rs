//! DuckLake attachment layer (completed in milestone M7).
//!
//! Architecture: inputs are read from the attached `lake` catalog via
//! cross-catalog SELECTs; all `__dedupe__*` intermediates live in the native
//! temp catalog (DuckLake cannot host temp tables); final outputs are written
//! back to the lake in dedicated single-catalog transactions.

#[derive(Debug, Clone)]
pub struct DuckLakeConfig {
    /// Everything after `ducklake:` in the ATTACH string, verbatim — e.g.
    /// `meta.ducklake`, `sqlite:meta.db`, `postgres:dbname=lake host=...`.
    pub metadata: String,
    /// `DATA_PATH` attach option (Parquet data location).
    pub data_path: Option<String>,
    /// Catalog alias; never `USE`d — all references are fully qualified.
    pub alias: String,
    /// Schema within the lake catalog.
    pub schema: String,
    pub read_only: bool,
}

impl Default for DuckLakeConfig {
    fn default() -> Self {
        DuckLakeConfig {
            metadata: String::new(),
            data_path: None,
            alias: "lake".to_string(),
            schema: "main".to_string(),
            read_only: false,
        }
    }
}

/// Physical layout options for a lake write ([`write_to_lake_with`]).
///
/// DuckLake stores tables as Parquet files with a SQL catalog and has **no
/// secondary indexes** (`CREATE INDEX` is rejected — tables are columnar
/// files, not B-trees). The two levers it *does* offer are partitioning and
/// sort-on-write, both captured here:
///
/// - [`partition_by`](Self::partition_by): writes one Parquet subtree per
///   distinct value (`col=val/…`), so a reader filtering on that column scans
///   only matching files. Use it for columns downstream queries filter on —
///   `source_dataset`, a batch/date column, a cluster bucket. It does **not**
///   speed up the in-engine blocking join (which runs on native temp tables,
///   not the lake).
/// - [`order_by`](Self::order_by): physically sorts rows on write so each
///   Parquet file's min/max zone-maps are tight, letting range/equality
///   filters skip whole files. This is DuckLake's equivalent of a clustering
///   key — no separate index object, just well-ordered data plus statistics.
///
/// [`write_to_lake_with`]: super::Session::write_to_lake_with
#[derive(Debug, Clone, Default)]
pub struct LakeWriteOptions {
    /// Partition columns (column names; empty = unpartitioned single table).
    pub partition_by: Vec<String>,
    /// Sort columns applied on write (empty = no explicit ordering).
    pub order_by: Vec<String>,
}

impl LakeWriteOptions {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.partition_by.is_empty() && self.order_by.is_empty()
    }
}
