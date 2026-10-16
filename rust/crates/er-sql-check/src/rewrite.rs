//! The rewrite step: what DataFusion's parser cannot read is removed before
//! planning. On the lake that is DuckLake's `AT (VERSION => $snapshot)`
//! time-travel clause (sqlparser 0.59 cannot parse the `=>`); on the control
//! store it will also cover `RETURNING` / `ON CONFLICT` / `FOR UPDATE` (Phase 3).
//! Each removal records what it stripped so a later pass can enforce the rule it
//! stood for (E006, the pinned-snapshot rule).

use std::sync::LazyLock;

use regex::Regex;

/// `AT (VERSION => $snapshot)` or `AT (VERSION => 42)`, tolerant of whitespace.
static SNAPSHOT_PIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+AT\s*\(\s*VERSION\s*=>\s*(\$?\w+)\s*\)").unwrap());

/// What the rewrite step removed from a statement, so later passes can check the
/// rules those clauses stood for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rewritten {
    /// The SQL with unplannable clauses removed, ready for the parser.
    pub sql: String,
    /// The snapshot parameter a lake read pinned (`$snapshot` -> `snapshot`), or
    /// `None` if the statement carried no `AT (VERSION => ..)` clause.
    pub snapshot_param: Option<String>,
}

/// Remove a DuckLake `AT (VERSION => ..)` clause before planning, recording the
/// snapshot parameter name. The snapshot is spliced into the statement text at
/// emit time, never bound, because DuckLake does not accept a parameter inside
/// `AT (VERSION => ..)`.
#[must_use]
pub fn rewrite_lake(sql: &str) -> Rewritten {
    if let Some(caps) = SNAPSHOT_PIN.captures(sql) {
        let snapshot_param = caps
            .get(1)
            .map(|m| m.as_str().trim_start_matches('$').to_string());
        let sql = SNAPSHOT_PIN.replace(sql, "").to_string();
        Rewritten {
            sql,
            snapshot_param,
        }
    } else {
        Rewritten {
            sql: sql.to_string(),
            snapshot_param: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_a_named_snapshot_pin() {
        let r = rewrite_lake(
            "SELECT entity_id FROM lake.main.golden_records AT (VERSION => $snapshot) ORDER BY entity_id",
        );
        assert_eq!(r.snapshot_param.as_deref(), Some("snapshot"));
        assert!(!r.sql.contains("AT"));
        assert!(
            r.sql
                .contains("FROM lake.main.golden_records ORDER BY entity_id")
        );
    }

    #[test]
    fn strips_a_literal_snapshot_pin() {
        let r = rewrite_lake("SELECT entity_id FROM lake.main.golden_records AT (VERSION => 42)");
        assert_eq!(r.snapshot_param.as_deref(), Some("42"));
        assert!(!r.sql.contains("VERSION"));
    }

    #[test]
    fn leaves_a_statement_without_a_pin_unchanged() {
        let sql = "SELECT entity_id FROM lake.main.golden_records";
        let r = rewrite_lake(sql);
        assert_eq!(r.snapshot_param, None);
        assert_eq!(r.sql, sql);
    }
}
