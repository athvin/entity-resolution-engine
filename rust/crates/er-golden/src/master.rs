//! Master-record election policies (port of `er.golden.master`,
//! the `er-server-control-plane` skill's `references/backend-design.md` §10).
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

/// The Account survivable columns: `acct_golden_records`' surviving values,
/// in that relation's column order. `most_complete` counts these for an
/// Account member, as [`GOLDEN_SURVIVABLE_COLUMNS`] for a person.
pub const ACCT_SURVIVABLE_COLUMNS: [&str; 7] = [
    "company_name",
    "domain",
    "phone_e164",
    "addr_street",
    "addr_city",
    "addr_region",
    "addr_postal",
];

/// How many survivable attributes a member carries, over the S5 column set.
fn completeness_expr() -> String {
    completeness_over(&GOLDEN_SURVIVABLE_COLUMNS)
}

/// [`completeness_expr`] over an explicit column set.
fn completeness_over(columns: &[&str]) -> String {
    let terms = columns
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

/// The full merge-plan statement for one `(policy, protect, since)` shape — a
/// governed generator: its output is planned by the analyser's vector tests
/// (every policy × protect on/off × since on/off), the way the mart assembly
/// SQL is. One row per MEMBER of each affected >=2-member entity (flat — the
/// read path groups in Rust; a `list(.. ORDER BY ..)` aggregate would not
/// plan), ordered by `(entity_id, record_key)`, with the elected master and the
/// golden preview repeated per row.
///
/// Reads live by design (the Python-export parity rule). Bound parameters, in
/// order: `$1 = since` (text timestamp, only when `since`), then the group
/// LIMIT (`$2`, or `$1` when `since` is absent).
///
/// `protect` injects the overlay override topology: a member with an active
/// `protect` overlay forces itself master for its entity (min(record_key)
/// tie-break), overriding the policy election.
///
/// # Errors
/// [`MasterElectionError`] as [`masters_cte_sql`].
pub fn merge_plans_sql(
    policy: &str,
    schema_qualifier: &str,
    source_ranks: Option<&BTreeMap<String, i64>>,
    protect: bool,
    since: bool,
) -> Result<String, MasterElectionError> {
    let masters_sql = masters_cte_sql(policy, schema_qualifier, source_ranks)?;
    let since_filter = if since {
        "AND occurred_at >= $1::timestamp "
    } else {
        ""
    };
    let limit_param = if since { "$2" } else { "$1" };
    let masters_cte = if protect {
        // `protected` picks, per affected entity, the active-protected member
        // (min(record_key) is a deterministic tie-break when several members of
        // one entity are protected). COALESCE lets it win over the policy master.
        format!(
            "masters_policy AS ( {masters_sql} ), \
             protected AS ( \
               SELECT m.entity_id, min(m.record_key) AS master_key \
               FROM {schema_qualifier}.entity_membership m \
               JOIN affected a ON a.entity_id = m.entity_id \
               JOIN {schema_qualifier}.record_overlays o \
                 ON o.record_key = m.record_key AND o.kind = 'protect' AND o.active \
               GROUP BY m.entity_id \
             ), \
             masters AS ( \
               SELECT p.entity_id, COALESCE(pr.master_key, p.master_key) AS master_key \
               FROM masters_policy p \
               LEFT JOIN protected pr ON pr.entity_id = p.entity_id \
             )"
        )
    } else {
        format!("masters AS ( {masters_sql} )")
    };
    Ok(format!(
        "WITH affected AS ( \
           SELECT DISTINCT entity_id FROM {schema_qualifier}.entity_events \
           WHERE event_type IN ('merged', 'member_added') {since_filter}\
         ), \
         {masters_cte}, \
         plan_groups AS ( \
           SELECT m.entity_id, ms.master_key, count(*) AS member_count \
           FROM {schema_qualifier}.entity_membership m \
           JOIN masters ms ON ms.entity_id = m.entity_id \
           GROUP BY m.entity_id, ms.master_key HAVING count(*) >= 2 \
           ORDER BY m.entity_id LIMIT {limit_param} \
         ) \
         SELECT gr.entity_id, gr.master_key, gr.member_count, \
                m.source_system, m.source_record_id, m.record_key, \
                g.given_name, g.family_name, g.email, g.phone_e164 \
         FROM plan_groups gr \
         JOIN {schema_qualifier}.entity_membership m ON m.entity_id = gr.entity_id \
         LEFT JOIN {schema_qualifier}.golden_records g ON g.id = gr.entity_id \
         ORDER BY gr.entity_id, m.record_key"
    ))
}

/// Master election restricted to an ELIGIBLE subset of each entity's members,
/// per `(entity_id, group_key)` (#993). The writeback preview elects a master
/// among the members that may be written to one provider connection and
/// object — electing over the whole entity and discarding a foreign master
/// would leave groups masterless, or worse, quietly pick a different policy.
/// Same policy vocabulary and orderings as [`masters_cte_sql`]; this is that
/// election with a narrower input, not a second one.
///
/// A governed generator: plan-vector-tested in er-sql-check for every policy
/// × protect. Bound parameters, in order: `$1` entity ids, `$2` group keys,
/// `$3` record keys — three equally long lists joined by `chr(31)`, one triple
/// per eligible member. Output: `(entity_id, group_key, master_key)`, one row
/// per group, ordered by `(entity_id, group_key)`.
///
/// # Errors
/// [`MasterElectionError`] as [`masters_cte_sql`].
pub fn scoped_masters_sql(
    policy: &str,
    schema_qualifier: &str,
    source_ranks: Option<&BTreeMap<String, i64>>,
    protect: bool,
) -> Result<String, MasterElectionError> {
    scoped_masters_over(
        policy,
        schema_qualifier,
        source_ranks,
        protect,
        Relations {
            std: "int_std_records",
            lineage: "golden_lineage",
            survivable: &GOLDEN_SURVIVABLE_COLUMNS,
        },
    )
}

/// The Account twin of [`scoped_masters_sql`] (#1309): the same election,
/// parameters and output, over `acct_int_std_records` and
/// `acct_golden_lineage`, with `most_complete` counting
/// [`ACCT_SURVIVABLE_COLUMNS`]. Membership and overlays are shared relations.
/// One lake holds one entity type, so a caller picks by the config's.
///
/// # Errors
/// [`MasterElectionError`] as [`masters_cte_sql`].
pub fn acct_scoped_masters_sql(
    policy: &str,
    schema_qualifier: &str,
    source_ranks: Option<&BTreeMap<String, i64>>,
    protect: bool,
) -> Result<String, MasterElectionError> {
    scoped_masters_over(
        policy,
        schema_qualifier,
        source_ranks,
        protect,
        Relations {
            std: "acct_int_std_records",
            lineage: "acct_golden_lineage",
            survivable: &ACCT_SURVIVABLE_COLUMNS,
        },
    )
}

/// The entity-type-specific relations the scoped election reads.
struct Relations<'a> {
    std: &'a str,
    lineage: &'a str,
    survivable: &'a [&'a str],
}

fn scoped_masters_over(
    policy: &str,
    schema_qualifier: &str,
    source_ranks: Option<&BTreeMap<String, i64>>,
    protect: bool,
    rel: Relations<'_>,
) -> Result<String, MasterElectionError> {
    let Relations {
        std,
        lineage,
        survivable,
    } = rel;
    if !MASTER_ELECTION_POLICIES.contains(&policy) {
        return Err(MasterElectionError(format!(
            "{policy:?} is not a master-election policy; the vocabulary is {MASTER_ELECTION_POLICIES:?}"
        )));
    }
    let eligible = "eligible AS ( \
           SELECT unnest(string_split($1, chr(31))) AS entity_id, \
                  unnest(string_split($2, chr(31))) AS group_key, \
                  unnest(string_split($3, chr(31))) AS record_key \
         )";
    let ranked = if policy == "most_attributes" {
        format!(
            "SELECT entity_id, group_key, record_key, \
                    row_number() OVER ( \
                      PARTITION BY entity_id, group_key ORDER BY won_attributes DESC, record_key \
                    ) AS rank \
             FROM ( \
               SELECT e.entity_id, e.group_key, e.record_key, count(l.record_key) AS won_attributes \
               FROM eligible e \
               LEFT JOIN {schema_qualifier}.{lineage} l \
                 ON l.entity_id = e.entity_id AND l.record_key = e.record_key \
               GROUP BY e.entity_id, e.group_key, e.record_key \
             )"
        )
    } else {
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
            "most_recent" => "COALESCE(s.updated_at_source, s.ingested_at) DESC".to_string(),
            "oldest" => "COALESCE(s.updated_at_source, s.ingested_at) ASC".to_string(),
            _ => format!("{} DESC", completeness_over(survivable)),
        };
        format!(
            "SELECT e.entity_id, e.group_key, e.record_key, \
                    row_number() OVER ( \
                      PARTITION BY e.entity_id, e.group_key ORDER BY {order}, e.record_key \
                    ) AS rank \
             FROM eligible e \
             JOIN {schema_qualifier}.entity_membership m \
               ON m.entity_id = e.entity_id AND m.record_key = e.record_key \
             JOIN {schema_qualifier}.{std} s ON s.record_key = e.record_key"
        )
    };
    let elected = format!(
        "elected AS ( SELECT entity_id, group_key, record_key AS master_key FROM ( {ranked} ) WHERE rank = 1 )"
    );
    if protect {
        Ok(format!(
            "WITH {eligible}, {elected}, \
             protected AS ( \
               SELECT e.entity_id, e.group_key, min(e.record_key) AS master_key \
               FROM eligible e \
               JOIN {schema_qualifier}.record_overlays o \
                 ON o.record_key = e.record_key AND o.kind = 'protect' AND o.active \
               GROUP BY e.entity_id, e.group_key \
             ) \
             SELECT el.entity_id, el.group_key, COALESCE(p.master_key, el.master_key) AS master_key \
             FROM elected el \
             LEFT JOIN protected p ON p.entity_id = el.entity_id AND p.group_key = el.group_key \
             ORDER BY el.entity_id, el.group_key"
        ))
    } else {
        Ok(format!(
            "WITH {eligible}, {elected} \
             SELECT entity_id, group_key, master_key FROM elected \
             ORDER BY entity_id, group_key"
        ))
    }
}
