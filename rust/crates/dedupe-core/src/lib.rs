//! dedupe-core: a Fellegi-Sunter probabilistic record-linkage engine,
//! narrowly scoped to DuckDB/DuckLake execution.
//!
//! Regression contract: given the same settings JSON and input data, the
//! engine must reproduce the committed golden baselines exactly (trained
//! parameters, match weights/probabilities, clusters) — see
//! `fixtures/golden/`.

pub mod accuracy;
pub mod blocking;
pub mod blocking_analysis;
pub mod clustering;
pub mod comparison;
pub mod error;
pub mod incremental;
pub mod linker;
pub mod model;
pub mod predict;
pub mod profile;
pub mod scoring;
pub mod session;
pub mod settings;
pub mod sql;
pub mod stages;
pub mod training;

pub use error::{DedupeError, Result};
pub use linker::Linker;
pub use model::TrainedModel;
pub use session::ducklake::{DuckLakeConfig, LakeWriteOptions};
pub use session::{Session, SessionOptions};
pub use settings::Settings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
