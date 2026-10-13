//! Master-record election policies (port of `er.golden.master`,
//! docs/backend-design.md §10).
//!
//! Survivorship elects field *values*; an external merge needs a master
//! *record* — the one member of an entity that survives when the others fold
//! into it. This module is the single owner of that election so the merge-plan
//! export and any future apply/steward path elect the same master.
//!
//! Normative properties (ported verbatim): the policy vocabulary is closed
//! ([`MASTER_ELECTION_POLICIES`]); every policy is a total order ending in the
//! mandatory `record_key` tiebreak (S6.1 — a policy that can tie depends on scan
//! order); `most_attributes` is the default and reproduces the shipped election
//! byte-for-byte (the member contributing the most golden attributes per
//! `golden_lineage`, ties to the lowest `record_key`; members with no lineage
//! row are never elected). The module renders SQL text — the read path owns its
//! connection, snapshot pinning, and schema qualification. The rendered fragment
//! references an `affected(entity_id)` CTE the caller must define.

use std::collections::BTreeMap;
use std::fmt;

use er_schema::columns::GOLDEN_SURVIVABLE_COLUMNS;

/// The closed policy vocabulary, in documentation order.
pub const MASTER_ELECTION_POLICIES: [&str; 5] = [
    "most_attributes",
    "source_priority",
    "most_recent",
    "oldest",
    "most_complete",
];

/// The default policy — the election the merge-plan export shipped with.
pub const DEFAULT_MASTER_ELECTION_POLICY: &str = "most_attributes";

/// The sentinel an unranked source sorts under: after every ranked source. A
/// missing rank is config/lake skew; demoting it never elects a record the
/// config no longer describes while ranked members exist. (`2^31 - 1`.)
const UNRANKED_SENTINEL: i64 = 2_147_483_647;

/// An election the vocabulary does not describe; the caller maps it to 422.
#[derive(Debug, Clone)]
pub struct MasterElectionError(pub String);

impl fmt::Display for MasterElectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MasterElectionError {}

/// A single-quoted SQL string literal with embedded quotes doubled.
fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// The `CASE` expression ordering members by their source's priority rank.
fn rank_case(source_ranks: &BTreeMap<String, i64>) -> String {
    let arms = source_ranks
        .iter()
        .map(|(source, rank)| format!("WHEN {} THEN {rank}", quote_literal(source)))
        .collect::<Vec<_>>()
        .join(" ");
    format!("COALESCE(CASE m.source_system {arms} END, {UNRANKED_SENTINEL})")
}

/// How many survivable attributes a member carries, over the S5 column set.
fn completeness_expr() -> String {
    let terms = GOLDEN_SURVIVABLE_COLUMNS
        .iter()
        .map(|c| format!("(CASE WHEN s.{c} IS NOT NULL THEN 1 ELSE 0 END)"))
        .collect::<Vec<_>>()
        .join(" + ");
    format!("({terms})")
}

/// The body of a `masters(entity_id, master_key)` CTE for one policy.
///
/// `schema_qualifier` (e.g. `lake.main`) is interpolated verbatim, so it MUST
/// come from the caller's own configuration, never user input. `source_ranks`
/// (`source_system -> priority_rank`) is required by — and only by — the
/// `source_priority` policy. The rendered SQL references an `affected(entity_id)`
/// CTE the caller defines.
///
/// # Errors
/// [`MasterElectionError`] if `policy` is unknown, or `source_priority` was asked
/// for without `source_ranks`.
pub fn masters_cte_sql(
    policy: &str,
    schema_qualifier: &str,
    source_ranks: Option<&BTreeMap<String, i64>>,
) -> Result<String, MasterElectionError> {
    if !MASTER_ELECTION_POLICIES.contains(&policy) {
        return Err(MasterElectionError(format!(
            "{policy:?} is not a master-election policy; the vocabulary is {MASTER_ELECTION_POLICIES:?}"
        )));
    }

    if policy == "most_attributes" {
        // The shipped election, byte-compatible: lineage contribution counts.
        return Ok(format!(
            "SELECT entity_id, record_key AS master_key \
             FROM ( \
               SELECT entity_id, record_key, \
                      row_number() OVER ( \
                        PARTITION BY entity_id ORDER BY won_attributes DESC, record_key \
                      ) AS rank \
               FROM ( \
                 SELECT l.entity_id, l.record_key, count(*) AS won_attributes \
                 FROM {schema_qualifier}.golden_lineage l \
                 JOIN affected a ON a.entity_id = l.entity_id \
                 GROUP BY l.entity_id, l.record_key \
               ) \
             ) WHERE rank = 1"
        ));
    }

    let order = match policy {
        "source_priority" => {
            let ranks = source_ranks.ok_or_else(|| {
                MasterElectionError(
                    "the source_priority policy needs the tenant's sources.<name>.priority_rank \
                     mapping"
                        .to_string(),
                )
            })?;
            format!("{} ASC", rank_case(ranks))
        }
        // `ingested_at` is NOT NULL (S5), so the COALESCE never yields NULL.
        "most_recent" => "COALESCE(s.updated_at_source, s.ingested_at) DESC".to_string(),
        "oldest" => "COALESCE(s.updated_at_source, s.ingested_at) ASC".to_string(),
        _ => format!("{} DESC", completeness_expr()), // most_complete
    };

    Ok(format!(
        "SELECT entity_id, master_key \
         FROM ( \
           SELECT m.entity_id, m.record_key AS master_key, \
                  row_number() OVER ( \
                    PARTITION BY m.entity_id ORDER BY {order}, m.record_key \
                  ) AS rank \
           FROM {schema_qualifier}.entity_membership m \
           JOIN affected a ON a.entity_id = m.entity_id \
           JOIN {schema_qualifier}.int_std_records s ON s.record_key = m.record_key \
         ) WHERE rank = 1"
    ))
}
