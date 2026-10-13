//! Blocking rules: raw SQL join conditions over `l.`/`r.`-prefixed tables,
//! with the cross-rule dedup trick ("AND NOT coalesce((previous rule), false)").

use crate::sql::columns;

#[derive(Debug, Clone, PartialEq)]
pub struct BlockingRule {
    /// 0-based position; emitted as the `match_key` string literal.
    pub index: usize,
    /// Raw SQL join condition, passed through verbatim.
    pub sql: String,
    /// Array columns to UNNEST before blocking (membership blocking). Empty =
    /// an ordinary scalar equi/filter rule.
    pub arrays_to_explode: Vec<String>,
}

impl BlockingRule {
    pub fn is_exploding(&self) -> bool {
        !self.arrays_to_explode.is_empty()
    }
}

impl BlockingRule {
    /// Base column names referenced by this rule (table prefixes dropped),
    /// used for EM comparison deactivation.
    pub fn referenced_columns(&self) -> crate::error::Result<Vec<String>> {
        let cols = columns::columns_used(&self.sql, &format!("blocking rule {}", self.index))?;
        let mut out: Vec<String> = Vec::new();
        for c in cols {
            if !out.contains(&c.column) {
                out.push(c.column);
            }
        }
        Ok(out)
    }
}
