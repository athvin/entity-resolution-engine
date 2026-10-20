//! `__dedupe__blocked_id_pairs`: one UNION ALL arm per blocking rule, with
//! `match_key` = rule index as a string literal and the cross-rule dedup
//! trick. Matches golden `predict_01___dedupe__blocked_id_pairs.sql`.
//!
//! v5 blocks on `__dedupe__concat` (TF is not needed for blocking).

use super::UidExpr;
use crate::blocking::BlockingRule;
use crate::settings::json::LinkType;

pub const BLOCKED_ID_PAIRS: &str = "__dedupe__blocked_id_pairs";

/// The WHERE condition by link type (`_sql_gen_where_condition`):
/// - dedupe_only / link_and_dedupe: `where {uid_l} < {uid_r}`
/// - link_only: additionally `and l.{sd} != r.{sd}`
/// - two-dataset link_only: `where 1=1` (handled when link support lands)
fn where_condition(uid: &UidExpr, link_type: LinkType, source_dataset_col: &str) -> String {
    let base = format!("where {} < {}", uid.render("l"), uid.render("r"));
    match link_type {
        LinkType::DedupeOnly | LinkType::LinkAndDedupe => base,
        LinkType::LinkOnly => {
            format!("{base} and l.\"{source_dataset_col}\" != r.\"{source_dataset_col}\"")
        }
    }
}

/// A right-side hash-chunk filter: `and (hash(<uid_r>) % n) = i`, keeping only
/// pairs whose *right* record falls in chunk `i` of `n`. Because the dedupe
/// where-clause fixes `uid_l < uid_r`, every candidate pair has a single right
/// record, so partitioning on it assigns each pair to exactly one chunk —
/// unioning all chunks reproduces the full result with no duplicates. `None`
/// or `n <= 1` yields no filter (byte-identical to the unchunked path).
/// DuckDB's `hash()` is unsigned, so plain `%` needs no sign handling.
fn right_chunk_filter(uid: &UidExpr, chunk: Option<(u32, u32)>) -> String {
    match chunk {
        Some((i, n)) if n > 1 => format!("\nand (hash({}) % {n}) = {i}", uid.render("r")),
        _ => String::new(),
    }
}

fn exclude_preceding_sql(rule: &BlockingRule, preceding: &[&BlockingRule]) -> String {
    let _ = rule;
    if preceding.is_empty() {
        return String::new();
    }
    // A *scalar* preceding rule excludes its own pairs via
    // `coalesce((<sql>),false)`; an *exploding* preceding rule contributes the
    // literal `false` (its pairs live in a separate marginal table and are
    // deduplicated downstream by `min(match_key)`, not by this predicate).
    let clauses: Vec<String> = preceding
        .iter()
        .map(|r| {
            if r.is_exploding() {
                "false".to_string()
            } else {
                format!("coalesce(({}),false)", r.sql)
            }
        })
        .collect();
    format!("\nAND NOT ({})", clauses.join(" OR "))
}

/// Cross-table blocked id pairs (`two_dataset_link_only` semantics): inner
/// join left x right on each rule with `where 1=1` (no `uid_l < uid_r`), the
/// same string `match_key` and "AND NOT preceding" dedup. Used by
/// incremental linkage (predict_between), where left and right are disjoint
/// record collections.
pub fn blocked_id_pairs_cross_sql(
    rules: &[BlockingRule],
    uid: &UidExpr,
    left_table: &str,
    right_table: &str,
) -> String {
    let fallback;
    let rules: &[BlockingRule] = if rules.is_empty() {
        fallback = [BlockingRule {
            index: 0,
            sql: "1=1".to_string(),
            arrays_to_explode: Vec::new(),
        }];
        &fallback
    } else {
        rules
    };

    let arms: Vec<String> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| {
            let preceding: Vec<&BlockingRule> = rules[..i].iter().collect();
            format!(
                "select\n'{match_key}' as match_key,\n{uid_l} as join_key_l,\n{uid_r} as join_key_r\nfrom {left_table} as l\ninner join {right_table} as r\non\n({rule_sql})\nwhere 1=1{exclude}",
                match_key = rule.index,
                uid_l = uid.render("l"),
                uid_r = uid.render("r"),
                rule_sql = rule.sql,
                exclude = exclude_preceding_sql(rule, &preceding),
            )
        })
        .collect();
    arms.join("\n\nUNION ALL\n\n")
}

/// `__dedupe__blocked_id_pairs` arms over `input_table`. `marginals` maps each
/// *exploding* rule's index to the physical name of its pre-materialized
/// marginal id-pairs table (see [`marginal_exploded_id_pairs_sql`]); scalar
/// rules ignore it. When any exploding rule is present the caller must wrap
/// the UNION-ALL in [`dedup_non_unique_sql`] to collapse duplicate pairs by
/// `min(match_key)`.
pub fn blocked_id_pairs_sql(
    rules: &[BlockingRule],
    uid: &UidExpr,
    link_type: LinkType,
    source_dataset_col: &str,
    input_table: &str,
    marginals: &[(usize, String)],
    right_chunk: Option<(u32, u32)>,
) -> String {
    // No rules -> cartesian product via a literal 1=1 rule.
    let fallback;
    let rules: &[BlockingRule] = if rules.is_empty() {
        fallback = [BlockingRule {
            index: 0,
            sql: "1=1".to_string(),
            arrays_to_explode: Vec::new(),
        }];
        &fallback
    } else {
        rules
    };

    let arms: Vec<String> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| {
            if rule.is_exploding() {
                // Exploding arm reads its pre-materialized marginal table
                // directly; the `where`/`exclude` logic was already applied
                // when that table was built.
                let marginal = marginals
                    .iter()
                    .find(|(idx, _)| *idx == rule.index)
                    .map(|(_, name)| name.as_str())
                    .expect("exploding rule missing its marginal id-pairs table");
                return format!(
                    "select\n'{match_key}' as match_key,\njoin_key_l,\njoin_key_r\nfrom {marginal}",
                    match_key = rule.index,
                );
            }
            let preceding: Vec<&BlockingRule> = rules[..i].iter().collect();
            format!(
                "select\n'{match_key}' as match_key,\n{uid_l} as join_key_l,\n{uid_r} as join_key_r\nfrom {input_table} as l\ninner join {input_table} as r\non\n({rule_sql})\n{where}{exclude}{chunk}",
                match_key = rule.index,
                uid_l = uid.render("l"),
                uid_r = uid.render("r"),
                rule_sql = rule.sql,
                where = where_condition(uid, link_type, source_dataset_col),
                exclude = exclude_preceding_sql(rule, &preceding),
                chunk = right_chunk_filter(uid, right_chunk),
            )
        })
        .collect();
    arms.join("\n\nUNION ALL\n\n")
}

/// `__dedupe__blocked_id_pairs_non_unique` (templated name of the un-deduped
/// UNION-ALL when exploding rules are present).
pub const BLOCKED_ID_PAIRS_NON_UNIQUE: &str = "__dedupe__blocked_id_pairs_non_unique";

/// Templated name for the UNNEST'd concat used to build an exploding rule's
/// marginal id-pairs table.
pub const DF_CONCAT_UNNESTED: &str = "__dedupe__concat_unnested";

/// Physical/templated name of an exploding rule's marginal id-pairs table.
pub fn marginal_exploded_table_name(match_key: usize) -> String {
    format!("__dedupe__marginal_exploded_ids_blocking_rule_mk_{match_key}")
}

/// Collapse `__dedupe__blocked_id_pairs_non_unique` to unique pairs, keeping
/// the smallest `match_key` per pair (lexical string min —
/// `match_key` is a string literal, never cast to int).
pub fn dedup_non_unique_sql(non_unique_table: &str) -> String {
    format!(
        "select\nmin(match_key) as match_key,\njoin_key_l,\njoin_key_r\nfrom {non_unique_table}\ngroup by join_key_l, join_key_r"
    )
}

/// Recursive array explosion, mirroring DuckDB's `explode_arrays_sql`: each
/// `unnest(col) as col` wraps the previous level as a subquery, so multiple
/// array columns cross-product element-wise. NULL/empty arrays yield zero rows
/// naturally (so records with no tokens drop out of blocking).
pub fn explode_arrays_sql(
    tbl: &str,
    columns_to_explode: &[String],
    other_columns_to_retain: &[String],
) -> String {
    if columns_to_explode.is_empty() {
        let cols: Vec<String> = other_columns_to_retain
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect();
        return format!("select {} from {tbl}", cols.join(","));
    }
    let mut remaining = columns_to_explode.to_vec();
    let col = remaining.pop().expect("non-empty");
    let mut selects = vec![format!("unnest(\"{col}\") as \"{col}\"")];
    selects.extend(other_columns_to_retain.iter().map(|c| format!("\"{c}\"")));
    selects.extend(remaining.iter().map(|c| format!("\"{c}\"")));
    let mut retain = other_columns_to_retain.to_vec();
    retain.push(col);
    format!(
        "select {} from ({})",
        selects.join(","),
        explode_arrays_sql(tbl, &remaining, &retain)
    )
}

/// Columns the UNNEST'd concat must carry for an exploding rule: the unique id
/// (and source dataset, if any), plus every base column referenced by the rule
/// and all preceding rules, dedup-preserving-order, minus the array columns
/// being exploded.
pub fn blocking_retain_columns(
    preceding: &[&BlockingRule],
    rule: &BlockingRule,
    uid_column: &str,
    source_dataset_col: Option<&str>,
) -> crate::error::Result<Vec<String>> {
    let mut cols: Vec<String> = Vec::new();
    if let Some(sd) = source_dataset_col {
        cols.push(sd.to_string());
    }
    cols.push(uid_column.to_string());
    for br in preceding.iter().copied().chain(std::iter::once(rule)) {
        for c in br.referenced_columns()? {
            if !cols.contains(&c) {
                cols.push(c);
            }
        }
    }
    cols.retain(|c| !rule.arrays_to_explode.contains(c));
    Ok(cols)
}

/// One exploding rule's marginal id-pairs table: distinct `(join_key_l,
/// join_key_r)` from a self-join of the UNNEST'd concat on the rule's scalar
/// condition, with the dedupe `uid_l < uid_r` where-clause and the
/// "AND NOT preceding" exclusion already applied. Materialized before the main
/// blocked-pairs pipeline; the exploding arm then reads it directly.
pub fn marginal_exploded_id_pairs_sql(
    rule: &BlockingRule,
    preceding: &[&BlockingRule],
    uid: &UidExpr,
    link_type: LinkType,
    source_dataset_col: &str,
    unnested_table: &str,
    right_chunk: Option<(u32, u32)>,
) -> String {
    format!(
        "select distinct\n{uid_l} as join_key_l,\n{uid_r} as join_key_r\nfrom {t} as l\ninner join {t} as r\non\n({rule_sql})\n{where}{exclude}{chunk}",
        uid_l = uid.render("l"),
        uid_r = uid.render("r"),
        t = unnested_table,
        rule_sql = rule.sql,
        where = where_condition(uid, link_type, source_dataset_col),
        exclude = exclude_preceding_sql(rule, preceding),
        chunk = right_chunk_filter(uid, right_chunk),
    )
}
