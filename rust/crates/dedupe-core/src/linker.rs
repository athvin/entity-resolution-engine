//! High-level orchestration façade over a `Session`:
//! train (prior -> u -> EM sessions), predict, cluster.

use crate::clustering::{ClusterOptions, cluster_at_threshold, clustered_with_input_data};
use crate::error::Result;
use crate::model::TrainedModel;
use crate::predict;
use crate::scoring::match_weight_to_prob;
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::training::em::{EmOptions, EmSessionResult, run_em_session};
use crate::training::estimate_u::{UOptions, estimate_u};
use crate::training::prior::estimate_prior;

pub struct TrainConfig {
    pub deterministic_rules: Vec<String>,
    pub recall: f64,
    pub u_max_pairs: f64,
    pub em_rules: Vec<String>,
}

pub struct TrainOutcome {
    pub prior: f64,
    pub em_sessions: Vec<EmSessionResult>,
}

/// Orchestrates the full linkage workflow against one registered input table.
pub struct Linker<'a> {
    session: &'a Session,
    model: TrainedModel,
    concat: TableHandle,
    tf_tables: Vec<TableHandle>,
}

impl<'a> Linker<'a> {
    /// Build a linker over an already-registered input table (e.g. from a CSV
    /// or a lake table). Materializes `__splink__df_concat` and the TF tables.
    pub fn new(session: &'a Session, settings: Settings, input: &TableHandle) -> Result<Self> {
        Self::new_multi(session, settings, &[(input, "")])
    }

    /// Build a linker over one or more registered inputs. For link types, each
    /// `(table, display_name)` becomes a `source_dataset`; for dedupe a single
    /// input is used.
    pub fn new_multi(
        session: &'a Session,
        settings: Settings,
        inputs: &[(&TableHandle, &str)],
    ) -> Result<Self> {
        let model = TrainedModel::new(settings);
        let concat = predict::materialize_concat_multi(session, inputs, &model.settings)?;
        let tf_tables = predict::materialize_tf_tables(session, &model.settings, &concat)?;
        Ok(Linker {
            session,
            model,
            concat,
            tf_tables,
        })
    }

    pub fn model(&self) -> &TrainedModel {
        &self.model
    }

    pub fn into_model(self) -> TrainedModel {
        self.model
    }

    pub fn session_ref(&self) -> &'a Session {
        self.session
    }

    pub fn settings_ref(&self) -> &Settings {
        &self.model.settings
    }

    pub fn concat_ref(&self) -> &TableHandle {
        &self.concat
    }

    pub fn tf_tables_ref(&self) -> &[TableHandle] {
        &self.tf_tables
    }

    /// Run the full training sequence: prior, then u, then EM sessions.
    pub fn train(&mut self, cfg: &TrainConfig) -> Result<TrainOutcome> {
        let prior = estimate_prior(
            self.session,
            &mut self.model,
            &self.concat,
            &cfg.deterministic_rules,
            cfg.recall,
        )?;
        estimate_u(
            self.session,
            &mut self.model,
            &self.concat,
            UOptions {
                max_pairs: cfg.u_max_pairs,
                ..Default::default()
            },
        )?;
        let mut em_sessions = Vec::new();
        for rule in &cfg.em_rules {
            let result = run_em_session(
                self.session,
                &mut self.model,
                &self.concat,
                &self.tf_tables,
                rule,
                EmOptions::default(),
            )?;
            em_sessions.push(result);
        }
        Ok(TrainOutcome { prior, em_sessions })
    }

    /// Predict, optionally filtering to a match-probability threshold.
    pub fn predict(&self, threshold_match_probability: Option<f64>) -> Result<TableHandle> {
        let threshold_mw = threshold_match_probability.and_then(prob_threshold_to_mw);
        let blocked =
            predict::materialize_blocked_pairs(self.session, &self.model.settings, &self.concat)?;
        let pipeline = predict::predict_pipeline(
            &self.model.settings,
            &self.concat,
            &self.tf_tables,
            &blocked,
            threshold_mw,
        )?;
        self.session.materialize(&pipeline)
    }

    /// As [`predict`](Self::predict), but scores the candidate pairs in
    /// `num_chunks` bounded-memory passes (`hash(uid_r) % num_chunks`) and
    /// unions them. The result is identical to `predict`; `num_chunks <= 1`
    /// dispatches straight to it. Reuses this linker's already-materialized
    /// concat and TF tables across passes. See PERFORMANCE.md.
    pub fn predict_chunked(
        &self,
        threshold_match_probability: Option<f64>,
        num_chunks: u32,
    ) -> Result<TableHandle> {
        if num_chunks <= 1 {
            return self.predict(threshold_match_probability);
        }
        let threshold_mw = threshold_match_probability.and_then(prob_threshold_to_mw);
        let mut acc: Option<TableHandle> = None;
        for i in 0..num_chunks {
            let blocked = predict::materialize_blocked_pairs_chunk(
                self.session,
                &self.model.settings,
                &self.concat,
                Some((i, num_chunks)),
            )?;
            let pipeline = predict::predict_pipeline(
                &self.model.settings,
                &self.concat,
                &self.tf_tables,
                &blocked,
                threshold_mw,
            )?;
            let part = self.session.materialize(&pipeline)?;
            self.session.drop_table(&blocked)?;
            match &acc {
                None => acc = Some(part),
                Some(a) => {
                    self.session.execute_batch(&format!(
                        "INSERT INTO {} SELECT * FROM {}",
                        a.physical, part.physical
                    ))?;
                    self.session.drop_table(&part)?;
                }
            }
        }
        acc.ok_or_else(|| {
            crate::error::DedupeError::Other("predict produced no chunks".to_string())
        })
    }

    /// Chunked predict that **streams each pass straight to a lake table**
    /// rather than accumulating the full result in a temp table — peak memory
    /// stays at one pass (~`1/num_chunks` of the pairs) end to end. Pass 0
    /// creates the target (empty + partitioned, or directly) and writes its
    /// rows; each later pass appends and is flushed to Parquet before the next
    /// begins. The written table is identical to [`predict`](Self::predict)'s
    /// output (order aside). Returns the row count.
    ///
    /// Each pass commits its own snapshot (committing is what frees the memory),
    /// so a run of N chunks leaves N snapshots — run `dedupe maintenance
    /// --checkpoint` afterwards to compact them. Requires an attached writable
    /// lake. See PERFORMANCE.md.
    pub fn predict_chunked_to_lake(
        &self,
        target: &str,
        threshold_match_probability: Option<f64>,
        num_chunks: u32,
        lake_opts: &crate::session::ducklake::LakeWriteOptions,
    ) -> Result<i64> {
        let threshold_mw = threshold_match_probability.and_then(prob_threshold_to_mw);
        let n = num_chunks.max(1);
        let mut created = false;
        for i in 0..n {
            let chunk = if n > 1 { Some((i, n)) } else { None };
            let blocked = predict::materialize_blocked_pairs_chunk(
                self.session,
                &self.model.settings,
                &self.concat,
                chunk,
            )?;
            let pipeline = predict::predict_pipeline(
                &self.model.settings,
                &self.concat,
                &self.tf_tables,
                &blocked,
                threshold_mw,
            )?;
            let part = self.session.materialize(&pipeline)?;
            self.session.drop_table(&blocked)?;
            if created {
                self.session
                    .lake_append(&part, target, &lake_opts.order_by)?;
            } else {
                // First pass seeds the table (handles partitioning + the first
                // batch of rows via the standard create-or-replace write).
                self.session.write_to_lake_with(&part, target, lake_opts)?;
                created = true;
            }
            self.session.drop_table(&part)?;
        }
        let back = self
            .session
            .register_lake_table(target, "__splink__stream_count")?;
        self.session.count(&back.physical)
    }

    /// Cluster predictions at a probability threshold and join back to inputs.
    pub fn cluster(
        &self,
        predictions: &TableHandle,
        threshold_match_probability: f64,
    ) -> Result<TableHandle> {
        let clusters = cluster_at_threshold(
            self.session,
            &self.model.settings,
            &self.concat,
            predictions,
            ClusterOptions {
                threshold_match_probability,
            },
        )?;
        clustered_with_input_data(self.session, &self.model.settings, &self.concat, &clusters)
    }
}

/// Convert a probability threshold to a match-weight threshold for the
/// predict filter; a threshold of 0 means "no filter".
fn prob_threshold_to_mw(p: f64) -> Option<f64> {
    if p <= 0.0 {
        None
    } else {
        // Inverse of match_weight_to_prob: mw = log2(p / (1 - p)).
        let _ = match_weight_to_prob; // documents the inverse relationship
        Some((p / (1.0 - p)).log2())
    }
}
