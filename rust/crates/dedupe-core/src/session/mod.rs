//! Session: owns one DuckDB connection, materializes pipelines as temp
//! tables with run-scoped physical names, and registers input data. The
//! DuckLake attachment layer lives in `ducklake` (all `__splink__*`
//! intermediates stay in the native temp catalog regardless — DuckLake cannot
//! host temp tables).

pub mod ducklake;

use std::cell::Cell;
use std::path::{Path, PathBuf};

use duckdb::Connection;

use crate::error::{DedupeError, Result};
use crate::sql::pipeline::CtePipeline;

/// Quote each name as a DuckDB identifier (`"name"`, embedded `"` doubled)
/// and join with `, ` — for building column lists from caller-supplied names.
fn quote_ident_list(cols: &[String]) -> String {
    cols.iter()
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone)]
pub struct TableHandle {
    /// Templated pipeline name, e.g. `__splink__df_concat`.
    pub templated: String,
    /// Actual table/view name in the database.
    pub physical: String,
    /// Column names in table order.
    pub columns: Vec<String>,
}

/// Tuning knobs applied to a [`Session`]'s DuckDB connection at open time.
///
/// The engine is a SQL generator — DuckDB does the actual compute — so these
/// map directly onto DuckDB configuration and are the primary levers for
/// scaling across CPUs and bounding memory:
///
/// - [`threads`](Self::threads): DuckDB parallelizes each query across this
///   many worker threads. More cores → faster blocking/scoring/clustering with
///   no code change. `None` = DuckDB's default (all logical cores).
/// - [`memory_limit`](Self::memory_limit): hard cap on in-memory buffers; above
///   it DuckDB spills intermediates to [`temp_directory`](Self::temp_directory)
///   rather than OOM-ing, so a workload larger than RAM still completes.
/// - [`preserve_insertion_order`](Self::preserve_insertion_order): `false` lets
///   DuckDB drop row-order bookkeeping, cutting peak memory on large scans/joins
///   (safe here — every stage is explicitly ordered when order matters).
///
/// Use [`SessionOptions::deterministic`] for deterministic single-thread runs
/// (what the golden regression tests use) and [`SessionOptions::performance`] for a
/// throughput-oriented production default.
#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// Worker threads per query. `None` = DuckDB default (all logical cores).
    /// Golden tests pin `Some(1)` so float aggregation order matches the
    /// golden baselines.
    pub threads: Option<u32>,
    /// In-memory buffer cap, e.g. `"8GB"` / `"75%"`. `None` = DuckDB default
    /// (~80% of RAM). Beyond it DuckDB spills to `temp_directory`.
    pub memory_limit: Option<String>,
    /// Directory for spilling intermediates larger than `memory_limit`.
    /// `None` = DuckDB default. Set this to run larger-than-RAM workloads.
    pub temp_directory: Option<String>,
    /// Cap on the spill directory, e.g. `"100GB"`. `None` = DuckDB default.
    pub max_temp_directory_size: Option<String>,
    /// `Some(false)` drops insertion-order tracking to lower peak memory.
    /// `None` = DuckDB default (`true`).
    pub preserve_insertion_order: Option<bool>,
    /// DuckDB extensions to `LOAD` at open, so their functions are usable in
    /// comparison/blocking SQL — e.g. `"rapidfuzz"` (token_sort/token_set/
    /// partial ratios) or `"splink_udfs"` (soundex, double_metaphone). The core
    /// string metrics (`jaro_winkler_similarity`, `levenshtein`,
    /// `damerau_levenshtein`, `jaccard`) are built in and need no extension.
    pub extensions: Vec<String>,
    /// DuckDB `extension_directory` — where pre-installed extension binaries
    /// live. Bake these into the container image so loading needs no network
    /// (`None` = DuckDB default, `~/.duckdb/extensions`).
    pub extension_directory: Option<String>,
    /// If `true`, `INSTALL <ext> FROM community` before loading (needs network;
    /// for local/dev use). Default `false`: `LOAD` only, assuming the binaries
    /// are already present/baked.
    pub install_extensions: bool,
}

impl SessionOptions {
    /// Deterministic single-thread configuration used by the golden regression
    /// tests: `threads = 1` keeps floating-point aggregation order stable so
    /// results match the golden baselines byte-for-byte.
    #[must_use]
    pub fn deterministic() -> Self {
        Self {
            threads: Some(1),
            ..Self::default()
        }
    }

    /// Throughput-oriented configuration: use all CPU cores, relax
    /// insertion-order bookkeeping to cut peak memory, and spill to
    /// `temp_directory` so workloads larger than memory still complete.
    #[must_use]
    pub fn performance(temp_directory: impl Into<String>) -> Self {
        Self {
            temp_directory: Some(temp_directory.into()),
            preserve_insertion_order: Some(false),
            ..Self::default()
        }
    }

    /// Set the worker-thread count (builder style).
    #[must_use]
    pub fn with_threads(mut self, threads: u32) -> Self {
        self.threads = Some(threads);
        self
    }

    /// Set the in-memory buffer cap, e.g. `"8GB"` (builder style).
    #[must_use]
    pub fn with_memory_limit(mut self, limit: impl Into<String>) -> Self {
        self.memory_limit = Some(limit.into());
        self
    }

    /// Set the spill directory for larger-than-memory workloads (builder style).
    #[must_use]
    pub fn with_temp_directory(mut self, dir: impl Into<String>) -> Self {
        self.temp_directory = Some(dir.into());
        self
    }
}

pub struct Session {
    conn: Connection,
    run_id: String,
    seq: Cell<u64>,
    lake: Option<LakeAttachment>,
    /// A spill directory this session created (because the caller supplied
    /// none) and is responsible for removing on drop.
    owned_temp_dir: Option<PathBuf>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // Best-effort: DuckDB removes its own spill files on connection close;
        // this clears the dedicated directory we created for them.
        if let Some(dir) = &self.owned_temp_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[derive(Debug, Clone)]
struct LakeAttachment {
    alias: String,
    schema: String,
}

/// A DuckLake catalog snapshot (version + commit timestamp).
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub version: i64,
    pub timestamp: String,
}

impl Session {
    pub fn open_in_memory(mut opts: SessionOptions) -> Result<Session> {
        let conn = Connection::open_in_memory()?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let run_id = format!("{nanos:08x}");

        // An in-memory DuckDB keeps everything resident unless it has somewhere
        // to spill. Give every session a dedicated spill directory when the
        // caller didn't name one: this bounds peak memory (DuckDB evicts to
        // disk under pressure) and guarantees no two sessions share a temp dir
        // (a known corruption hazard, and a `Drop` race between sessions).
        // Removed on drop. The pid + a process-wide counter make it unique even
        // across sessions opened concurrently (e.g. parallel tests).
        let owned_temp_dir = if opts.temp_directory.is_none() {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);
            let n = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("dedupe-spill-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir)
                .map_err(|e| DedupeError::Other(format!("creating spill directory: {e}")))?;
            opts.temp_directory = Some(dir.to_string_lossy().into_owned());
            Some(dir)
        } else {
            None
        };

        Self::apply_options(&conn, &opts)?;
        Self::load_extensions(&conn, &opts)?;
        Ok(Session {
            conn,
            run_id,
            seq: Cell::new(0),
            lake: None,
            owned_temp_dir,
        })
    }

    /// Apply [`SessionOptions`] as DuckDB `SET` statements. String-valued
    /// options are single-quote-escaped before interpolation.
    fn apply_options(conn: &Connection, opts: &SessionOptions) -> Result<()> {
        fn quote(v: &str) -> String {
            format!("'{}'", v.replace('\'', "''"))
        }
        let mut stmts: Vec<String> = Vec::new();
        if let Some(threads) = opts.threads {
            stmts.push(format!("SET threads = {threads};"));
        }
        if let Some(limit) = &opts.memory_limit {
            stmts.push(format!("SET memory_limit = {};", quote(limit)));
        }
        if let Some(dir) = &opts.temp_directory {
            stmts.push(format!("SET temp_directory = {};", quote(dir)));
        }
        if let Some(size) = &opts.max_temp_directory_size {
            stmts.push(format!("SET max_temp_directory_size = {};", quote(size)));
        }
        if let Some(preserve) = opts.preserve_insertion_order {
            stmts.push(format!("SET preserve_insertion_order = {preserve};"));
        }
        if let Some(dir) = &opts.extension_directory {
            stmts.push(format!("SET extension_directory = {};", quote(dir)));
        }
        if !stmts.is_empty() {
            conn.execute_batch(&stmts.join("\n"))?;
        }
        Ok(())
    }

    /// `LOAD` (optionally `INSTALL`) the configured extensions so their
    /// functions are available to generated SQL. Extension names are validated
    /// as bare identifiers (no SQL injection). With `install_extensions` off
    /// (the default) this is pure `LOAD` — no network — so bake the binaries
    /// into the image and point `extension_directory` at them.
    fn load_extensions(conn: &Connection, opts: &SessionOptions) -> Result<()> {
        for ext in &opts.extensions {
            if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(DedupeError::settings(format!(
                    "invalid extension name `{ext}` (expected a bare identifier)"
                )));
            }
            if opts.install_extensions {
                conn.execute_batch(&format!("INSTALL {ext} FROM community;"))?;
            }
            conn.execute_batch(&format!("LOAD {ext};"))?;
        }
        Ok(())
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Attach a DuckLake catalog. Intermediates still live in the native temp
    /// catalog; the lake is used only to read inputs and write outputs. The
    /// catalog is never `USE`d — all lake references are fully qualified.
    pub fn attach_ducklake(&mut self, cfg: &ducklake::DuckLakeConfig) -> Result<()> {
        self.conn
            .execute_batch("INSTALL ducklake; LOAD ducklake;")?;
        let mut options = Vec::new();
        if let Some(dp) = &cfg.data_path {
            options.push(format!("DATA_PATH '{}'", dp.replace('\'', "''")));
        }
        if cfg.read_only {
            options.push("READ_ONLY".to_string());
        }
        // Pin catalog format: never auto-migrate behind our back.
        options.push("AUTOMATIC_MIGRATION false".to_string());
        let opts = if options.is_empty() {
            String::new()
        } else {
            format!(" ({})", options.join(", "))
        };
        self.conn.execute_batch(&format!(
            "ATTACH 'ducklake:{}' AS {}{opts};",
            cfg.metadata.replace('\'', "''"),
            cfg.alias
        ))?;
        if !cfg.read_only {
            // Deterministic Parquet output (no catalog-inlined small writes).
            self.conn.execute_batch(&format!(
                "CALL {}.set_option('data_inlining_row_limit', 0);",
                cfg.alias
            ))?;
        }
        self.lake = Some(LakeAttachment {
            alias: cfg.alias.clone(),
            schema: cfg.schema.clone(),
        });
        Ok(())
    }

    fn lake(&self) -> Result<&LakeAttachment> {
        self.lake
            .as_ref()
            .ok_or_else(|| DedupeError::Other("no DuckLake catalog attached".to_string()))
    }

    /// Register a table from the attached lake catalog under a templated name
    /// (via a temp view, so downstream handling is uniform).
    pub fn register_lake_table(&self, table: &str, templated: &str) -> Result<TableHandle> {
        let lake = self.lake()?;
        let qualified = format!("{}.{}.{}", lake.alias, lake.schema, table);
        self.register_table(&qualified, templated)
    }

    /// Write a table into the lake in its own transaction (one writable
    /// catalog per transaction). Returns the snapshot version created by the
    /// commit (DuckLake records one snapshot per transaction).
    pub fn write_to_lake(&self, src: &TableHandle, target: &str) -> Result<i64> {
        self.write_to_lake_with(src, target, &ducklake::LakeWriteOptions::default())
    }

    /// Write `src` to the lake with an explicit physical layout
    /// ([`LakeWriteOptions`](ducklake::LakeWriteOptions)): partition columns
    /// and/or a sort order. Partitioning uses the empty-create → `SET
    /// PARTITIONED BY` → ordered `INSERT` sequence so the *initial* load is
    /// partitioned (DuckLake only partitions data written after the ALTER).
    /// All within one transaction (DuckLake allows one writable catalog per
    /// transaction). Returns the resulting snapshot version.
    pub fn write_to_lake_with(
        &self,
        src: &TableHandle,
        target: &str,
        opts: &ducklake::LakeWriteOptions,
    ) -> Result<i64> {
        let lake = self.lake()?;
        let full = format!("{}.{}.{target}", lake.alias, lake.schema);
        let order_clause = if opts.order_by.is_empty() {
            String::new()
        } else {
            format!(" ORDER BY {}", quote_ident_list(&opts.order_by))
        };

        let sql = if opts.partition_by.is_empty() {
            format!(
                "BEGIN; CREATE OR REPLACE TABLE {full} AS SELECT * FROM {}{order_clause}; COMMIT;",
                src.physical
            )
        } else {
            // Partitioning only applies to data written after `SET PARTITIONED
            // BY`, so create the table empty (schema only), set partitioning,
            // then insert the rows (ordered if requested).
            format!(
                "BEGIN; \
                 CREATE OR REPLACE TABLE {full} AS SELECT * FROM {src} WHERE false; \
                 ALTER TABLE {full} SET PARTITIONED BY ({parts}); \
                 INSERT INTO {full} SELECT * FROM {src}{order_clause}; \
                 COMMIT;",
                src = src.physical,
                parts = quote_ident_list(&opts.partition_by),
            )
        };
        self.conn.execute_batch(&sql)?;
        self.latest_snapshot_version()
    }

    /// Append `src` to an existing lake table (one auto-committed snapshot),
    /// optionally ordered on write. Rows route to partitions automatically if
    /// the target is partitioned. Used by streaming/chunked writes, where each
    /// call flushes its Parquet and frees memory before the next pass.
    pub fn lake_append(&self, src: &TableHandle, target: &str, order_by: &[String]) -> Result<()> {
        let lake = self.lake()?;
        let order = if order_by.is_empty() {
            String::new()
        } else {
            format!(" ORDER BY {}", quote_ident_list(order_by))
        };
        self.conn.execute_batch(&format!(
            "INSERT INTO {}.{}.{target} SELECT * FROM {}{order};",
            lake.alias, lake.schema, src.physical
        ))?;
        Ok(())
    }

    /// The most recent snapshot version in the attached lake.
    pub fn latest_snapshot_version(&self) -> Result<i64> {
        let lake = self.lake()?;
        Ok(self.conn.query_row(
            &format!(
                "SELECT max(snapshot_id) FROM ducklake_snapshots('{}')",
                lake.alias
            ),
            [],
            |r| r.get::<_, i64>(0),
        )?)
    }

    /// Whether a lake relation exists in the attached catalog — the pipeline or
    /// steward drain may not have created it yet. Catalog introspection, one of
    /// the deliberate non-statement reads (the relation NAMES stay governed by
    /// the registry/entity gates).
    pub fn lake_table_exists(&self, table: &str) -> Result<bool> {
        let lake = self.lake()?;
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM duckdb_tables() \
             WHERE database_name = ? AND schema_name = ? AND table_name = ?",
            duckdb::params![lake.alias, lake.schema, table],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// `Some(row_count)` for an existing lake relation (by bare name), `None`
    /// when it has not been created yet. Catalog introspection + count — the
    /// doctor's presence/size probe.
    pub fn relation_row_count(&self, table: &str) -> Result<Option<i64>> {
        if !self.lake_table_exists(table)? {
            return Ok(None);
        }
        let lake = self.lake()?;
        let n: i64 = self.conn.query_row(
            &format!(
                "SELECT count(*) FROM {}.{}.{table}",
                lake.alias, lake.schema
            ),
            [],
            |r| r.get(0),
        )?;
        Ok(Some(n))
    }

    /// List all snapshots (version + ISO timestamp), newest last.
    pub fn list_snapshots(&self) -> Result<Vec<Snapshot>> {
        let lake = self.lake()?;
        let rows = self.query_strings(&format!(
            "SELECT snapshot_id, snapshot_time::VARCHAR FROM ducklake_snapshots('{}') ORDER BY snapshot_id",
            lake.alias
        ))?;
        Ok(rows
            .into_iter()
            .map(|r| Snapshot {
                version: r[0].parse().unwrap(),
                timestamp: r[1].clone(),
            })
            .collect())
    }

    /// Register a lake table as it existed at a given snapshot version
    /// (time-travel read), under a templated name via a temp view.
    pub fn register_lake_table_at_version(
        &self,
        table: &str,
        templated: &str,
        version: i64,
    ) -> Result<TableHandle> {
        let lake = self.lake()?;
        let physical = self.next_physical(templated);
        self.conn.execute_batch(&format!(
            "CREATE TEMP VIEW {physical} AS SELECT * FROM {}.{}.{table} AT (VERSION => {version})",
            lake.alias, lake.schema
        ))?;
        let columns = self.table_columns(&physical)?;
        Ok(TableHandle {
            templated: templated.to_string(),
            physical,
            columns,
        })
    }

    // --- DuckLake maintenance (require an attached writable lake) -------------
    // Thin wrappers over DuckLake's catalog-maintenance functions. See
    // PERFORMANCE.md and the `dedupe maintenance` CLI subcommand.

    /// Set a DuckLake catalog option (persisted in the catalog), e.g.
    /// `("parquet_compression", "zstd")` or `("target_file_size", "512MB")`.
    pub fn set_lake_option(&self, key: &str, value: &str) -> Result<()> {
        let lake = self.lake()?;
        self.conn.execute_batch(&format!(
            "CALL {}.set_option('{}', '{}');",
            lake.alias,
            key.replace('\'', "''"),
            value.replace('\'', "''"),
        ))?;
        Ok(())
    }

    /// All DuckLake catalog options as `(option_name, value)` (global scope).
    pub fn lake_options(&self) -> Result<Vec<(String, String)>> {
        let lake = self.lake()?;
        let rows = self.query_strings(&format!(
            "SELECT option_name, value FROM {}.options() ORDER BY option_name",
            lake.alias
        ))?;
        Ok(rows
            .into_iter()
            .map(|r| (r[0].clone(), r.get(1).cloned().unwrap_or_default()))
            .collect())
    }

    /// Per-table file counts: `(table_name, file_count, delete_file_count)`.
    /// Useful before/after maintenance to see compaction effect.
    pub fn lake_table_stats(&self) -> Result<Vec<(String, i64, i64)>> {
        let lake = self.lake()?;
        let rows = self.query_strings(&format!(
            "SELECT table_name, file_count, delete_file_count \
             FROM ducklake_table_info('{}') ORDER BY table_name",
            lake.alias
        ))?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r[0].clone(),
                    r[1].parse().unwrap_or(0),
                    r[2].parse().unwrap_or(0),
                )
            })
            .collect())
    }

    /// Run a full `CHECKPOINT`: flush inlined data, expire snapshots, merge
    /// adjacent files, rewrite delete-heavy files, clean up and delete orphans
    /// — bounded by the catalog's `expire_older_than` / `delete_older_than` /
    /// `rewrite_delete_threshold` options.
    pub fn checkpoint_lake(&self) -> Result<()> {
        let lake = self.lake()?;
        self.conn
            .execute_batch(&format!("CHECKPOINT {};", lake.alias))?;
        Ok(())
    }

    /// Flush inlined (catalog-resident) rows out to Parquet data files.
    pub fn flush_inlined_data(&self) -> Result<()> {
        let lake = self.lake()?;
        self.conn.execute_batch(&format!(
            "CALL ducklake_flush_inlined_data('{}');",
            lake.alias
        ))?;
        Ok(())
    }

    /// Merge small adjacent data files (without expiring snapshots). Bound the
    /// work per table with `max_file_size` (bytes) and `max_compacted_files`
    /// for tiered compaction.
    pub fn merge_adjacent_files(
        &self,
        max_file_size: Option<u64>,
        max_compacted_files: Option<u64>,
    ) -> Result<()> {
        let lake = self.lake()?;
        let mut args = vec![format!("'{}'", lake.alias)];
        if let Some(m) = max_file_size {
            args.push(format!("max_file_size => {m}"));
        }
        if let Some(m) = max_compacted_files {
            args.push(format!("max_compacted_files => {m}"));
        }
        self.conn.execute_batch(&format!(
            "CALL ducklake_merge_adjacent_files({});",
            args.join(", ")
        ))?;
        Ok(())
    }

    /// Expire snapshots older than an interval, e.g. `"7 days"`. Keep this well
    /// above your longest running transaction.
    pub fn expire_snapshots(&self, older_than: &str) -> Result<()> {
        let lake = self.lake()?;
        // Cast to a zone-less TIMESTAMP: the bundled DuckDB omits ICU, so
        // `now()` (TIMESTAMP WITH TIME ZONE) has no `- INTERVAL` binding.
        self.conn.execute_batch(&format!(
            "CALL ducklake_expire_snapshots('{}', older_than => now()::TIMESTAMP - INTERVAL '{}');",
            lake.alias,
            older_than.replace('\'', "''"),
        ))?;
        Ok(())
    }

    /// Rewrite a table's data files whose deleted fraction exceeds the catalog's
    /// `rewrite_delete_threshold`, reclaiming space from heavy deletes/updates.
    pub fn rewrite_data_files(&self, table: &str) -> Result<()> {
        let lake = self.lake()?;
        self.conn.execute_batch(&format!(
            "CALL ducklake_rewrite_data_files('{}', '{}');",
            lake.alias,
            table.replace('\'', "''"),
        ))?;
        Ok(())
    }

    /// Delete data files no longer referenced by any live snapshot.
    pub fn cleanup_old_files(&self) -> Result<()> {
        let lake = self.lake()?;
        self.conn.execute_batch(&format!(
            "CALL ducklake_cleanup_old_files('{}', cleanup_all => true);",
            lake.alias
        ))?;
        Ok(())
    }

    /// Delete files present in the data path but unknown to the catalog.
    pub fn delete_orphaned_files(&self) -> Result<()> {
        let lake = self.lake()?;
        self.conn.execute_batch(&format!(
            "CALL ducklake_delete_orphaned_files('{}', cleanup_all => true);",
            lake.alias
        ))?;
        Ok(())
    }

    fn next_physical(&self, templated: &str) -> String {
        let seq = self.seq.get();
        self.seq.set(seq + 1);
        format!("{templated}_{}_{seq}", self.run_id)
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.conn.execute_batch(sql).map_err(DedupeError::from)
    }

    fn table_columns(&self, table: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(&format!("DESCRIBE SELECT * FROM {table}"))?;
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(cols)
    }

    /// Register a CSV file as a temp table. `column_types` (name -> DuckDB
    /// type) pins the schema.
    pub fn register_csv(
        &self,
        path: &Path,
        templated: &str,
        column_types: &[(&str, &str)],
    ) -> Result<TableHandle> {
        let physical = self.next_physical(templated);
        let path_str = path.to_string_lossy().replace('\'', "''");
        let sql = if column_types.is_empty() {
            // Keep dates/times as strings (DATE/TIME excluded from inference),
            // matching the string-typed ingestion the golden datasets expect —
            // otherwise a `dob` auto-typed as DATE breaks string comparisons like
            // `levenshtein(dob_l, dob_r)`.
            format!(
                "CREATE TEMP TABLE {physical} AS SELECT * FROM read_csv('{path_str}', \
                 auto_type_candidates = ['BIGINT', 'DOUBLE', 'VARCHAR'])"
            )
        } else {
            let cols = column_types
                .iter()
                .map(|(name, ty)| format!("'{name}': '{ty}'"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "CREATE TEMP TABLE {physical} AS SELECT * FROM read_csv('{path_str}', columns = {{{cols}}})"
            )
        };
        self.conn.execute_batch(&sql)?;
        let columns = self.table_columns(&physical)?;
        Ok(TableHandle {
            templated: templated.to_string(),
            physical,
            columns,
        })
    }

    /// Register an existing table or view (by qualified name) under a
    /// templated name via a temp view, so downstream handling is uniform.
    pub fn register_table(&self, qualified_name: &str, templated: &str) -> Result<TableHandle> {
        let physical = self.next_physical(templated);
        self.conn.execute_batch(&format!(
            "CREATE TEMP VIEW {physical} AS SELECT * FROM {qualified_name}"
        ))?;
        let columns = self.table_columns(&physical)?;
        Ok(TableHandle {
            templated: templated.to_string(),
            physical,
            columns,
        })
    }

    /// Render and execute a pipeline, materializing the result as a temp
    /// table named after the pipeline's final output.
    pub fn materialize(&self, pipeline: &CtePipeline) -> Result<TableHandle> {
        let templated = pipeline.output_name().to_string();
        let physical = self.next_physical(&templated);
        let sql = format!("CREATE TEMP TABLE {physical} AS\n{}", pipeline.render());
        self.conn.execute_batch(&sql).map_err(|e| {
            DedupeError::Other(format!("materializing {templated}: {e}\nSQL:\n{sql}"))
        })?;
        let columns = self.table_columns(&physical)?;
        Ok(TableHandle {
            templated,
            physical,
            columns,
        })
    }

    pub fn drop_table(&self, handle: &TableHandle) -> Result<()> {
        self.conn
            .execute_batch(&format!("DROP TABLE IF EXISTS {}", handle.physical))?;
        Ok(())
    }

    pub fn count(&self, table: &str) -> Result<i64> {
        let n = self
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                r.get::<_, i64>(0)
            })?;
        Ok(n)
    }

    pub fn query_scalar_f64(&self, sql: &str) -> Result<f64> {
        Ok(self.conn.query_row(sql, [], |r| r.get::<_, f64>(0))?)
    }

    /// Run a query and return all rows rendered as strings (for diagnostics
    /// and small result sets). NULL renders as "NULL".
    /// [`Self::query_strings`] under its scratch name: the sanctioned call for a
    /// product read over a SESSION TEMP relation (`__preview_src` and friends) —
    /// the CONTRIBUTING "session-temp scratch" exemption, made visible in the
    /// call itself. Product reads over persistent lake relations use declared
    /// entity statements instead (the `freeform_sql` gate enforces this).
    pub fn query_scratch_strings(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        self.query_strings(sql)
    }

    pub fn query_strings(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        let mut stmt = self.conn.prepare(sql)?;
        let ncols = {
            let rows = stmt.query([])?;
            let _ = rows;
            stmt.column_count()
        };
        let mut out = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let mut rendered = Vec::with_capacity(ncols);
            for i in 0..ncols {
                let v: duckdb::types::Value = row.get(i)?;
                rendered.push(render_value(&v));
            }
            out.push(rendered);
        }
        Ok(out)
    }
}

fn render_value(v: &duckdb::types::Value) -> String {
    use duckdb::types::Value;
    match v {
        Value::Null => "NULL".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(i) => i.to_string(),
        Value::SmallInt(i) => i.to_string(),
        Value::Int(i) => i.to_string(),
        Value::BigInt(i) => i.to_string(),
        Value::HugeInt(i) => i.to_string(),
        Value::UTinyInt(i) => i.to_string(),
        Value::USmallInt(i) => i.to_string(),
        Value::UInt(i) => i.to_string(),
        Value::UBigInt(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(f) => format!("{f:?}"),
        Value::Text(s) => s.clone(),
        other => format!("{other:?}"),
    }
}
