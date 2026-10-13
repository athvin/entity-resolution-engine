//! One builder per pipeline stage. All SQL-shape-sensitive code
//! lives here, written against the captured golden SQL in
//! `fixtures/golden/fake_1000/sql/`.

pub mod block;
pub mod concat;
pub mod score;
pub mod tf;
pub mod vectors;

/// Composite unique-id separator for multi-dataset link types.
pub const CONCAT_SEPARATOR: &str = "-__-";

/// Renders the unique-id expression for one side of a join (`l` or `r`).
#[derive(Debug, Clone, PartialEq)]
pub enum UidExpr {
    /// Single unique-id column (dedupe_only).
    Bare(String),
    /// `source_dataset || '-__-' || unique_id` (link types).
    Composite {
        source_dataset: String,
        unique_id: String,
    },
}

impl UidExpr {
    pub fn render(&self, table_alias: &str) -> String {
        match self {
            UidExpr::Bare(col) => format!("{table_alias}.\"{col}\""),
            UidExpr::Composite {
                source_dataset,
                unique_id,
            } => format!(
                "{table_alias}.\"{source_dataset}\" || '{CONCAT_SEPARATOR}' || {table_alias}.\"{unique_id}\""
            ),
        }
    }

    /// Expression with no table alias (e.g. selecting node ids from a single
    /// source table).
    pub fn render_bare(&self) -> String {
        match self {
            UidExpr::Bare(col) => format!("\"{col}\""),
            UidExpr::Composite {
                source_dataset,
                unique_id,
            } => format!("\"{source_dataset}\" || '{CONCAT_SEPARATOR}' || \"{unique_id}\""),
        }
    }

    /// Same expression over `_l`/`_r`-suffixed columns (edge tables).
    pub fn render_suffixed(&self, suffix: &str) -> String {
        match self {
            UidExpr::Bare(col) => format!("\"{col}_{suffix}\""),
            UidExpr::Composite {
                source_dataset,
                unique_id,
            } => format!(
                "\"{source_dataset}_{suffix}\" || '{CONCAT_SEPARATOR}' || \"{unique_id}_{suffix}\""
            ),
        }
    }
}

pub fn uid_expr_for(settings: &crate::settings::Settings) -> UidExpr {
    if settings.source_dataset_required() {
        UidExpr::Composite {
            source_dataset: settings.source_dataset_column.clone(),
            unique_id: settings.unique_id_column.clone(),
        }
    } else {
        UidExpr::Bare(settings.unique_id_column.clone())
    }
}
