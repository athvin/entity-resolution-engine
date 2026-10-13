//! S4.6 survivorship: the chain concatenator, the five rules, and the
//! deciding-rule attribution — a Rust port of `dbt/macros/survivorship/*.sql`
//! that emits byte-equivalent DuckDB. Each attribute's chain is a total order
//! over an entity's member records, terminating in the mandatory `record_key
//! ASC` tiebreak. `survivorship_decision_sql` returns one winner row per
//! `entity_id` (value + record_key + the name of the rule that decided it).

use anyhow::{Result, bail};
use er_config::TERMINAL_SURVIVORSHIP_RULE;

/// The S5 `golden_lineage.rule` value when the terminal tiebreak decided (a lone
/// candidate, or every rule tied).
pub const TIEBREAK_RULE_NAME: &str = "tiebreak_deterministic";

/// One rule's S4.6 ORDER BY terms as `(expression, direction)` pairs. `attribute`
/// is substituted only where S4.6 writes `<attr>` (the `validated` rule). Unknown
/// rules fail the render, naming the offender.
fn rule_terms(rule: &str, attribute: &str) -> Result<Vec<(String, String)>> {
    let terms = match rule {
        "source_priority" => vec![(
            "sources[source_system].priority_rank".to_string(),
            "ASC".to_string(),
        )],
        "recency" => vec![(
            "COALESCE(updated_at_source, ingested_at)".to_string(),
            "DESC".to_string(),
        )],
        "frequency" => vec![(
            "count(*) OVER (PARTITION BY entity_id, value)".to_string(),
            "DESC".to_string(),
        )],
        "completeness" => vec![
            ("(value IS NOT NULL)".to_string(), "DESC".to_string()),
            ("length(value)".to_string(), "DESC".to_string()),
        ],
        "validated" => vec![(format!("{attribute}_valid"), "DESC NULLS LAST".to_string())],
        other => bail!(
            "survivorship: unknown rule {other:?} (S4.6 defines source_priority, recency, \
             frequency, completeness, validated)"
        ),
    };
    Ok(terms)
}

/// The chain's rule names with any terminal element removed, matching
/// `survivorship_chain_without_terminal` — S6.1 already appends the terminal, so
/// the renderer strips it and re-appends exactly one.
fn chain_without_terminal(chain: &[String]) -> Vec<&str> {
    chain
        .iter()
        .map(|r| r.trim())
        .filter(|r| *r != TERMINAL_SURVIVORSHIP_RULE.trim())
        .collect()
}

/// A `survivorship_decision` SELECT: one row per `entity_id` with the rank-1
/// winner's `value` and `record_key`, and the deciding rule — the FIRST rule
/// whose sort key differs between the rank-1 and rank-2 rows, else
/// [`TIEBREAK_RULE_NAME`]. Every rule's terms are materialized as named keys in
/// `_keyed` so the ORDER BY and the attribution read the same expressions
/// (`frequency` is itself a window, which cannot nest inside a window's ORDER BY).
pub fn survivorship_decision_sql(
    attribute: &str,
    chain: &[String],
    relation: &str,
) -> Result<String> {
    let rules = chain_without_terminal(chain);

    let mut projections: Vec<String> = Vec::new();
    let mut ordering: Vec<String> = Vec::new();
    let mut comparisons: Vec<String> = Vec::new();
    for rule in &rules {
        let terms = rule_terms(rule, attribute)?;
        let mut differs: Vec<String> = Vec::new();
        for (idx, (expr, dir)) in terms.iter().enumerate() {
            let alias = format!("_k_{rule}_{idx}");
            projections.push(format!("{expr} AS {alias}"));
            ordering.push(format!("{alias} {dir}"));
            differs.push(format!("_w.{alias} IS DISTINCT FROM _r.{alias}"));
        }
        comparisons.push(format!("WHEN {} THEN '{rule}'", differs.join(" OR ")));
    }
    ordering.push(TERMINAL_SURVIVORSHIP_RULE.to_string());

    let projection_clause = if projections.is_empty() {
        String::new()
    } else {
        format!(",\n        {}", projections.join(",\n        "))
    };
    let mut comparison_arms = String::new();
    for c in &comparisons {
        comparison_arms.push_str("        ");
        comparison_arms.push_str(c);
        comparison_arms.push('\n');
    }

    Ok(format!(
        "WITH _keyed AS (
    SELECT
        *{projection_clause}
    FROM {relation}
),
_ranked AS (
    SELECT
        *,
        row_number() OVER (
            PARTITION BY entity_id
            ORDER BY {ordering}
        ) AS _rn
    FROM _keyed
),
_top AS (
    SELECT * FROM _ranked WHERE _rn <= 2
)
SELECT
    _w.entity_id,
    _w.value,
    _w.record_key,
    CASE
        WHEN _r.record_key IS NULL THEN '{TIEBREAK_RULE_NAME}'
{comparison_arms}        ELSE '{TIEBREAK_RULE_NAME}'
    END AS rule
FROM _top AS _w
LEFT JOIN _top AS _r
       ON _r.entity_id = _w.entity_id
      AND _r._rn = 2
WHERE _w._rn = 1",
        ordering = ordering.join(", "),
    ))
}
