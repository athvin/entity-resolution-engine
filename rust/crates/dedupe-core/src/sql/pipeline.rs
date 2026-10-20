//! CTE pipeline assembler. Queued `(sql, output_name)` tasks are assembled
//! into one `WITH ... <final select>` query; references to input tables'
//! templated names are rewritten to their physical names (longest name first,
//! identifier-boundary aware). References to earlier CTE outputs keep their
//! templated names — they ARE the CTE aliases.

use regex::Regex;

use crate::session::TableHandle;

#[derive(Debug, Clone)]
pub struct SqlTask {
    pub sql: String,
    pub output: String,
}

#[derive(Debug, Default)]
pub struct CtePipeline {
    tasks: Vec<SqlTask>,
    /// (templated_name, physical_name) of pre-materialized inputs.
    inputs: Vec<(String, String)>,
}

impl CtePipeline {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_input(mut self, handle: &TableHandle) -> Self {
        self.add_input(handle);
        self
    }

    pub fn add_input(&mut self, handle: &TableHandle) {
        self.inputs
            .push((handle.templated.clone(), handle.physical.clone()));
    }

    pub fn enqueue(&mut self, sql: impl Into<String>, output: impl Into<String>) {
        self.tasks.push(SqlTask {
            sql: sql.into(),
            output: output.into(),
        });
    }

    pub fn output_name(&self) -> &str {
        &self.tasks.last().expect("empty pipeline").output
    }

    /// Render the full `WITH ...` SELECT.
    pub fn render(&self) -> String {
        assert!(!self.tasks.is_empty(), "cannot render an empty pipeline");

        // Longest-first so a name that prefixes another is never clobbered.
        let mut inputs = self.inputs.clone();
        inputs.sort_by_key(|(t, _)| std::cmp::Reverse(t.len()));

        let rewrite = |sql: &str| -> String {
            let mut out = sql.to_string();
            for (templated, physical) in &inputs {
                let pattern = format!(r"\b{}\b", regex::escape(templated));
                let re = Regex::new(&pattern).expect("valid rewrite regex");
                out = re.replace_all(&out, physical.as_str()).into_owned();
            }
            out
        };

        if self.tasks.len() == 1 {
            return rewrite(&self.tasks[0].sql);
        }

        let ctes: Vec<String> = self.tasks[..self.tasks.len() - 1]
            .iter()
            .map(|t| format!("{} as (\n{}\n)", t.output, rewrite(&t.sql)))
            .collect();
        format!(
            "WITH\n{}\n{}",
            ctes.join(",\n\n"),
            rewrite(&self.tasks.last().unwrap().sql)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(templated: &str, physical: &str) -> TableHandle {
        TableHandle {
            templated: templated.to_string(),
            physical: physical.to_string(),
            columns: vec![],
        }
    }

    #[test]
    fn single_task_rewrites_inputs() {
        let mut p = CtePipeline::new().with_input(&handle("__dedupe__concat", "concat_abc123"));
        p.enqueue("select * from __dedupe__concat", "out");
        assert_eq!(p.render(), "select * from concat_abc123");
    }

    #[test]
    fn prefix_names_do_not_clobber() {
        let mut p = CtePipeline::new()
            .with_input(&handle("__dedupe__concat", "c_1"))
            .with_input(&handle("__dedupe__concat_with_tf", "ctf_1"));
        p.enqueue(
            "select * from __dedupe__concat join __dedupe__concat_with_tf using (id)",
            "out",
        );
        assert_eq!(p.render(), "select * from c_1 join ctf_1 using (id)");
    }

    #[test]
    fn multi_task_builds_with_clause() {
        let mut p = CtePipeline::new().with_input(&handle("__dedupe__input", "in_1"));
        p.enqueue("select * from __dedupe__input", "a");
        p.enqueue("select count(*) as n from a", "b");
        let sql = p.render();
        assert!(sql.starts_with("WITH\na as (\nselect * from in_1\n)\n"));
        assert!(sql.ends_with("select count(*) as n from a"));
    }
}
