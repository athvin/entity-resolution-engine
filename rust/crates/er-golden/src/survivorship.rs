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
        // `most_common` is a clearer alias of `frequency` (same ORDER BY term);
        // both tokens stay valid and keep their own lineage attribution.
        "frequency" | "most_common" => vec![(
            "count(*) OVER (PARTITION BY entity_id, value)".to_string(),
            "DESC".to_string(),
        )],
        "completeness" => vec![
            ("(value IS NOT NULL)".to_string(), "DESC".to_string()),
            ("length(value)".to_string(), "DESC".to_string()),
        ],
        "validated" => vec![(format!("{attribute}_valid"), "DESC NULLS LAST".to_string())],
        // --- Deep field-level rules (PR 1.6, Bucket A). Each is additive: it
        // only renders when a chain names it, so configs using the five rules
        // above are byte-identical. All are single-winner ORDER BY terms, so
        // lineage attribution + record assembly are unchanged. ---
        // Longest / shortest value (Cloudingo "most characters").
        "longest" => vec![("length(value)".to_string(), "DESC".to_string())],
        "shortest" => vec![("length(value)".to_string(), "ASC".to_string())],
        // Numeric extremes; non-numeric values cast to NULL and sort last.
        "numeric_max" => vec![(
            "try_cast(value AS DOUBLE)".to_string(),
            "DESC NULLS LAST".to_string(),
        )],
        "numeric_min" => vec![(
            "try_cast(value AS DOUBLE)".to_string(),
            "ASC NULLS LAST".to_string(),
        )],
        // Ranked value list, token-encoded `prioritized:v1|v2|v3` (precedent:
        // the `jaro_winkler:<T>` comparison level). Earlier = higher priority;
        // unlisted values sort last.
        p if p.starts_with("prioritized:") => {
            let cases = p["prioritized:".len()..]
                .split('|')
                .filter(|v| !v.is_empty())
                .enumerate()
                .map(|(i, v)| format!("WHEN '{}' THEN {i}", v.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(" ");
            let expr = if cases.is_empty() {
                "2147483647".to_string()
            } else {
                format!("CASE value {cases} ELSE 2147483647 END")
            };
            vec![(expr, "ASC".to_string())]
        }
        other => bail!(
            "survivorship: unknown rule {other:?} (S4.6 defines source_priority, recency, \
             frequency, completeness, validated; PR 1.6 adds longest, shortest, numeric_max, \
             numeric_min, most_common, prioritized:<v1|v2|...>)"
        ),
    };
    Ok(terms)
}

/// A SQL-identifier-safe alias fragment for a rule token: non-`[A-Za-z0-9_]`
/// become `_`. Existing simple rules (`source_priority`, …) are unchanged, so
/// their generated SQL stays byte-identical; parameterized tokens like
/// `prioritized:a|b` become `prioritized_a_b`.
fn alias_key(rule: &str) -> String {
    rule.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
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
        let key = alias_key(rule);
        for (idx, (expr, dir)) in terms.iter().enumerate() {
            let alias = format!("_k_{key}_{idx}");
            projections.push(format!("{expr} AS {alias}"));
            ordering.push(format!("{alias} {dir}"));
            differs.push(format!("_w.{alias} IS DISTINCT FROM _r.{alias}"));
        }
        comparisons.push(format!(
            "WHEN {} THEN '{}'",
            differs.join(" OR "),
            rule.replace('\'', "''")
        ));
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
