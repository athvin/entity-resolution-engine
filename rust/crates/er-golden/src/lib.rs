//! The golden-record (survivorship) stage (port of `dbt/models/marts/golden_*`
//! + `dbt/macros/survivorship/*`, DesignDoc.md S4.6).
//!
//! Survivorship elects, per entity and per attribute, which member record's
//! value survives — driven by the config's per-attribute rule chains
//! (`validated`, `source_priority`, `recency`, `frequency`, `completeness`,
//! terminating in the mandatory `record_key ASC` tiebreak). [`survivorship`]
//! renders the decision; [`lineage`] emits `golden_lineage` (who won + which rule
//! decided); [`records`] assembles the surviving values into `golden_records`.
//!
//! These are SQL generators run through dedupe-core's DuckDB session — the
//! dbt-replacement contract: identical SQL over identical inputs → identical
//! rows, gated against base_10's committed `lineage.csv` / `golden.csv`.

pub mod lineage;
pub mod master;
pub mod records;
pub mod survivorship;

pub use lineage::golden_lineage_sql;
pub use master::{DEFAULT_MASTER_ELECTION_POLICY, MASTER_ELECTION_POLICIES, masters_cte_sql};
pub use records::golden_records_sql;
pub use survivorship::survivorship_decision_sql;
