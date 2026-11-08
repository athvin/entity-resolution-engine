//! `__dedupe__hierarchy_ancestors`: the ancestor closure of a declared record
//! hierarchy (`settings.hierarchy`), one row per (record, ancestor) pair.
//!
//! Blocking's hierarchy conjunct is a pair predicate — "neither record is an
//! ancestor of the other" — but ancestry is transitive and a pair of rows
//! carries only one parent hop, so the closure has to be materialized before
//! the rules run. It reads `__dedupe__concat` (which is a plain projection of
//! the input, so `metadata`, `source_record_id` and `source_system` are all
//! present) and is depth-capped: the cap is both "how deep a real hierarchy
//! gets" and the cycle guard, because a corrupted tree that loops would
//! otherwise recurse forever.

use crate::settings::json::HierarchyJson;
use crate::stages::concat::DF_CONCAT;

pub const DF_HIERARCHY_ANCESTORS: &str = "__dedupe__hierarchy_ancestors";

/// The closure SQL, as one recursive CTE over `__dedupe__concat` (rewritten to
/// the physical table by the pipeline that runs this).
///
/// Edge semantics: a child's `metadata ->> '<parent key>'` equals its parent's
/// `source_record_id`, within one `source_system` — a parent reference is raw
/// source linkage, so it can only point at a record of the same system. A
/// record whose key is absent, or whose value matches nothing, contributes no
/// edge and is excluded from nothing.
///
/// The key is spliced into a single-quoted literal with quote doubling; the
/// compiler has already refused anything outside the identifier charset, so
/// this is defence in depth rather than the validation.
#[must_use]
pub fn ancestors_sql(hierarchy: &HierarchyJson) -> String {
    let key = hierarchy.parent_metadata_key.replace('\'', "''");
    let depth = hierarchy.max_depth.max(1);
    format!(
        "with recursive edges as (\n\
         \x20   select c.record_key as record_key, p.record_key as ancestor_key\n\
         \x20   from {DF_CONCAT} c\n\
         \x20   join {DF_CONCAT} p\n\
         \x20     on c.metadata ->> '{key}' = p.source_record_id\n\
         \x20    and c.source_system = p.source_system\n\
         \x20    and c.record_key <> p.record_key\n\
         ), anc as (\n\
         \x20   select record_key, ancestor_key, 1 as depth from edges\n\
         \x20   union all\n\
         \x20   select a.record_key, e.ancestor_key, a.depth + 1\n\
         \x20   from anc a\n\
         \x20   join edges e on e.record_key = a.ancestor_key\n\
         \x20   where a.depth < {depth}\n\
         )\n\
         select distinct record_key, ancestor_key from anc"
    )
}

/// The conjunct the compiler freezes into every blocking rule when a hierarchy
/// excludes: the pair is admitted only when NEITHER record sits on the other's
/// ancestor chain. Living beside the SQL it depends on, and re-exported to the
/// compiler, so the table name and the predicate cannot drift apart.
#[must_use]
pub fn exclusion_conjunct() -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM {DF_HIERARCHY_ANCESTORS} h \
         WHERE (h.record_key = l.record_key AND h.ancestor_key = r.record_key) \
         OR (h.record_key = r.record_key AND h.ancestor_key = l.record_key))"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_quoted_and_the_depth_capped() {
        let sql = ancestors_sql(&HierarchyJson {
            parent_metadata_key: "Parent'Id".to_string(),
            max_depth: 0,
        });
        assert!(sql.contains("->> 'Parent''Id'"), "{sql}");
        assert!(
            sql.contains("a.depth < 1"),
            "a zero cap still terminates: {sql}"
        );
    }
}
