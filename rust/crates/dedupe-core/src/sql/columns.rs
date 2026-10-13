//! Column extraction from raw SQL condition strings.
//!
//! Behavior notes:
//! - `get_columns_used_from_sql`: every Column node in the tree, skipping any
//!   column inside a lambda body, treating `col[idx]` subscripts as `col`.
//! - `_input_columns_used_by_sql_condition`: the above, with `_l`/`_r`
//!   suffixes stripped case-insensitively, deduped preserving order.
//! - `_is_exact_match` / `_exact_match_colnames`: condition must normalize to
//!   a conjunction where every conjunct is `col_l = col_r` (bare columns)
//!   with equal base names after unconditionally stripping the last 2 chars.

use std::ops::ControlFlow;

use sqlparser::ast::{BinaryOperator, Expr, Ident, visit_expressions};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;

use crate::error::{DedupeError, Result};

/// A column reference found in a SQL expression, with its optional table
/// qualifier (e.g. `l."first_name"` -> table `l`, column `first_name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnRef {
    pub table: Option<String>,
    pub column: String,
}

pub fn parse_expr(sql: &str, context: &str) -> Result<Expr> {
    Parser::new(&DuckDbDialect {})
        .try_with_sql(sql)
        .and_then(|mut p| p.parse_expr())
        .map_err(|e| DedupeError::SqlParse {
            context: context.to_string(),
            message: e.to_string(),
        })
}

/// All column references in source order (pre-order traversal), excluding any
/// that appear inside lambda bodies.
pub fn columns_used(sql: &str, context: &str) -> Result<Vec<ColumnRef>> {
    let expr = parse_expr(sql, context)?;
    let mut all: Vec<ColumnRef> = Vec::new();
    collect_columns(&expr, &mut all);

    // Subtract columns that appear under lambdas.
    let mut lambda_cols: Vec<ColumnRef> = Vec::new();
    let _ = visit_expressions(&expr, |e: &Expr| {
        if let Expr::Lambda(lf) = e {
            collect_columns(&lf.body, &mut lambda_cols);
        }
        ControlFlow::<()>::Continue(())
    });
    for lc in lambda_cols {
        if let Some(pos) = all.iter().position(|c| *c == lc) {
            all.remove(pos);
        }
    }
    Ok(all)
}

fn ident_name(ident: &Ident) -> String {
    ident.value.clone()
}

fn collect_columns(expr: &Expr, out: &mut Vec<ColumnRef>) {
    let _ = visit_expressions(expr, |e: &Expr| {
        match e {
            Expr::Identifier(ident) => out.push(ColumnRef {
                table: None,
                column: ident_name(ident),
            }),
            Expr::CompoundIdentifier(parts) if !parts.is_empty() => {
                let column = ident_name(parts.last().unwrap());
                let table = if parts.len() > 1 {
                    Some(ident_name(&parts[parts.len() - 2]))
                } else {
                    None
                };
                out.push(ColumnRef { table, column });
            }
            _ => {}
        }
        ControlFlow::<()>::Continue(())
    });
}

fn strip_lr_suffix(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with("_l") || lower.ends_with("_r") {
        &name[..name.len() - 2]
    } else {
        name
    }
}

/// Referenced base column
/// names with `_l`/`_r` stripped, deduped preserving order of appearance.
pub fn input_columns(sql_condition: &str, context: &str) -> Result<Vec<String>> {
    if is_else_condition(sql_condition) {
        return Ok(Vec::new());
    }
    let cols = columns_used(sql_condition, context)?;
    let mut seen = Vec::new();
    for c in cols {
        let base = strip_lr_suffix(&c.column).to_string();
        if !seen.contains(&base) {
            seen.push(base);
        }
    }
    Ok(seen)
}

pub fn is_else_condition(sql_condition: &str) -> bool {
    sql_condition.trim().eq_ignore_ascii_case("ELSE")
}

/// Splits a (possibly parenthesized) expression into its top-level AND
/// conjuncts.
fn conjuncts(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::Nested(inner) => conjuncts(inner),
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            let mut out = conjuncts(left);
            out.extend(conjuncts(right));
            out
        }
        other => vec![other],
    }
}

fn as_bare_column(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(ident) => Some(ident_name(ident)),
        Expr::Nested(inner) => as_bare_column(inner),
        _ => None,
    }
}

/// Exact-match detection: returns
/// `Some(base_columns)` (lowercased, one per conjunct) if every top-level AND
/// conjunct is an equality of two bare columns whose names match after
/// unconditionally stripping the final two characters; otherwise `None`.
pub fn exact_match_columns(sql_condition: &str, context: &str) -> Result<Option<Vec<String>>> {
    if is_else_condition(sql_condition) {
        return Ok(None);
    }
    let expr = parse_expr(sql_condition, context)?;
    let mut cols = Vec::new();
    for conjunct in conjuncts(&expr) {
        let Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } = conjunct
        else {
            return Ok(None);
        };
        let (Some(l), Some(r)) = (as_bare_column(left), as_bare_column(right)) else {
            return Ok(None);
        };
        // The last two characters are truncated unconditionally ([:-2]) and
        // compared; the whole condition is lowercased before parsing.
        if l.len() < 2 || r.len() < 2 {
            return Ok(None);
        }
        let l_base = l[..l.len() - 2].to_lowercase();
        let r_base = r[..r.len() - 2].to_lowercase();
        if l_base != r_base {
            return Ok(None);
        }
        if !cols.contains(&l_base) {
            cols.push(l_base);
        }
    }
    Ok(Some(cols))
}

/// Equi-join columns of a blocking rule written in table-prefixed form
/// (`l.col = r.col [AND ...]`): `Some(cols)` if every top-level AND conjunct is
/// an equality of two column references naming the same column; `None`
/// otherwise (e.g. the rule has a filter condition). Unlike
/// `exact_match_columns`, the operands are table-qualified (`l.`/`r.`) and the
/// column name is taken as-is (no `_l`/`_r` stripping).
pub fn equi_join_columns(sql: &str, context: &str) -> Result<Option<Vec<String>>> {
    let expr = parse_expr(sql, context)?;
    let col_name = |e: &Expr| -> Option<String> {
        match e {
            Expr::Identifier(id) => Some(ident_name(id)),
            Expr::CompoundIdentifier(parts) if !parts.is_empty() => {
                Some(ident_name(parts.last().unwrap()))
            }
            Expr::Nested(inner) => match inner.as_ref() {
                Expr::Identifier(id) => Some(ident_name(id)),
                Expr::CompoundIdentifier(parts) if !parts.is_empty() => {
                    Some(ident_name(parts.last().unwrap()))
                }
                _ => None,
            },
            _ => None,
        }
    };
    let mut cols = Vec::new();
    for conjunct in conjuncts(&expr) {
        let Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } = conjunct
        else {
            return Ok(None);
        };
        let (Some(l), Some(r)) = (col_name(left), col_name(right)) else {
            return Ok(None);
        };
        if l != r {
            return Ok(None);
        }
        if !cols.contains(&l) {
            cols.push(l);
        }
    }
    Ok(Some(cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equi_join_column_extraction() {
        assert_eq!(
            equi_join_columns(r#"l."first_name" = r."first_name""#, "t").unwrap(),
            Some(vec!["first_name".to_string()])
        );
        assert_eq!(
            equi_join_columns(
                r#"(l."first_name" = r."first_name") AND (l."city" = r."city")"#,
                "t"
            )
            .unwrap(),
            Some(vec!["first_name".to_string(), "city".to_string()])
        );
        assert_eq!(
            equi_join_columns(r#"l."dob" = r."dob" and l.x > r.y"#, "t").unwrap(),
            None
        );
    }

    #[test]
    fn extracts_columns_from_simple_equality() {
        assert_eq!(
            input_columns(r#""first_name_l" = "first_name_r""#, "t").unwrap(),
            vec!["first_name"]
        );
    }

    #[test]
    fn extracts_columns_from_function_call() {
        assert_eq!(
            input_columns(
                r#"jaro_winkler_similarity("first_name_l", "first_name_r") >= 0.92"#,
                "t"
            )
            .unwrap(),
            vec!["first_name"]
        );
    }

    #[test]
    fn ignores_string_literals() {
        assert_eq!(
            input_columns(
                r#"NULLIF(regexp_extract("email_l", '^[^@]+', 0), '') = NULLIF(regexp_extract("email_r", '^[^@]+', 0), '')"#,
                "t"
            )
            .unwrap(),
            vec!["email"]
        );
    }

    #[test]
    fn blocking_rule_columns_with_table_prefix() {
        let cols = columns_used(
            r#"(l."first_name" = r."first_name") AND (l."city" = r."city")"#,
            "t",
        )
        .unwrap();
        assert_eq!(cols.len(), 4);
        assert_eq!(cols[0].table.as_deref(), Some("l"));
        assert_eq!(cols[0].column, "first_name");
        assert_eq!(cols[3].column, "city");
    }

    #[test]
    fn exact_match_detection() {
        assert_eq!(
            exact_match_columns(r#""email_l" = "email_r""#, "t").unwrap(),
            Some(vec!["email".to_string()])
        );
        assert_eq!(
            exact_match_columns(
                r#"NULLIF(regexp_extract("email_l", '^[^@]+', 0), '') = NULLIF(regexp_extract("email_r", '^[^@]+', 0), '')"#,
                "t"
            )
            .unwrap(),
            None
        );
        assert_eq!(
            exact_match_columns(r#"("a_l" = "a_r") AND ("b_l" = "b_r")"#, "t").unwrap(),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            exact_match_columns(r#"jaro_winkler_similarity("x_l", "x_r") >= 0.9"#, "t").unwrap(),
            None
        );
        assert_eq!(exact_match_columns("ELSE", "t").unwrap(), None);
    }

    #[test]
    fn null_level_condition_columns() {
        assert_eq!(
            input_columns(r#""city_l" IS NULL OR "city_r" IS NULL"#, "t").unwrap(),
            vec!["city"]
        );
    }
}
