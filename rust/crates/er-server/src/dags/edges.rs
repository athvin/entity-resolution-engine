//! The lake-edge vocabulary (§3.2): each edge names the relation set a
//! committed snapshot hands downstream. `tests/dag_assembly.rs` asserts every
//! relation here is in `er_schema::model::all_specs()`, so an unknown relation
//! is an assembly-tier refusal, mirroring `declared_names.rs`.

use er_dag::LakeEdge;

/// What `ingest` lands.
pub struct Landed;
impl LakeEdge for Landed {
    const STABLE_NAME: &'static str = "landed";
    const RELATIONS: &'static [&'static str] = &["raw_records", "ingest_batches"];
}

/// What `standardize` rebuilds.
pub struct Standardized;
impl LakeEdge for Standardized {
    const STABLE_NAME: &'static str = "standardized";
    const RELATIONS: &'static [&'static str] = &["int_std_records"];
}

/// What `match` scores.
pub struct Scored;
impl LakeEdge for Scored {
    const STABLE_NAME: &'static str = "scored";
    const RELATIONS: &'static [&'static str] = &["match_scores", "scored_records"];
}

/// What `reconcile` resolves.
pub struct Resolved;
impl LakeEdge for Resolved {
    const STABLE_NAME: &'static str = "resolved";
    const RELATIONS: &'static [&'static str] = &["entities", "entity_membership", "entity_events"];
}

/// What `assemble` (golden + reviews + metrics + quality) produces.
pub struct Assembled;
impl LakeEdge for Assembled {
    const STABLE_NAME: &'static str = "assembled";
    const RELATIONS: &'static [&'static str] = &[
        "golden_records",
        "review_queue",
        "metric_snapshots",
        "metric_snapshot_sources",
    ];
}

/// What `profile` records: the shape of a corpus stage, its most frequent
/// values and the coded findings over them.
pub struct Profiled;
impl LakeEdge for Profiled {
    const STABLE_NAME: &'static str = "profiled";
    const RELATIONS: &'static [&'static str] =
        &["profile_runs", "column_profiles", "profile_findings"];
}

/// What `apply_staged` writes: the steward's labels landed on the lake.
pub struct Labeled;
impl LakeEdge for Labeled {
    const STABLE_NAME: &'static str = "labeled";
    const RELATIONS: &'static [&'static str] = &["assertions", "review_queue"];
}

/// Every edge's `(STABLE_NAME, RELATIONS)`, for the assembly test.
#[must_use]
pub fn all() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        (Landed::STABLE_NAME, Landed::RELATIONS),
        (Standardized::STABLE_NAME, Standardized::RELATIONS),
        (Scored::STABLE_NAME, Scored::RELATIONS),
        (Resolved::STABLE_NAME, Resolved::RELATIONS),
        (Assembled::STABLE_NAME, Assembled::RELATIONS),
        (Profiled::STABLE_NAME, Profiled::RELATIONS),
        (Labeled::STABLE_NAME, Labeled::RELATIONS),
    ]
}
