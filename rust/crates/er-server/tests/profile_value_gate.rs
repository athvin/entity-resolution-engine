//! The profile's value gate: which fields of a column profile a viewer may see.
//!
//! A profile is statistics, but five of its fields are not statistics at all —
//! they echo a value exactly as the corpus stores it. `top_values` is the most
//! frequent values of the column; `lex_min`/`lex_max` the lexically smallest and
//! largest; `num_min`/`num_max` the numeric extremes. On an email column those
//! are real addresses, on a name column real names, on a salary column one
//! person's pay.
//!
//! The gate shipped covering `top_values` alone, and its own doc comment called
//! that "the one field in a profile that carries personal data" — so the belief
//! was recorded as fact in the place most likely to be trusted. The other four
//! went to every viewer.
//!
//! What makes this tractable is that the *failure mode is silence*: nothing
//! errors when a value-bearing field reaches the wire, and no test fails, so
//! the only thing that would have caught it is somebody re-deriving the list by
//! hand. This file does that derivation mechanically instead, against
//! `entities/column_profile/entity.yaml` as embedded in the binary — so adding
//! a column to the entity fails this test until it is classified, and the
//! question "is this one a value?" gets asked exactly when it can be answered.

use er_server::read::{DERIVED_COLUMN_FIELDS, VALUE_BEARING_COLUMN_FIELDS};

/// The column names of one entity, read from the YAML compiled into the binary.
///
/// Scanned rather than deserialized: `er-server` does not depend on the entity
/// spec crate, and the embedded YAML is generator output, so its shape is
/// mechanical (`  - name: <ident>` at exactly two spaces of indent, only inside
/// `columns:`). A format change breaks this loudly — the list comes back empty
/// and every assertion below fails — rather than quietly returning a subset,
/// which is the failure that would matter.
fn entity_columns(entity: &str) -> Vec<String> {
    let Some((_, yaml)) = er_entities::catalog::ENTITY_YAML
        .iter()
        .find(|(name, _)| *name == entity)
    else {
        panic!("entity {entity} is not in the compiled catalog")
    };

    let mut names = Vec::new();
    let mut in_columns = false;
    for line in yaml.lines() {
        if line.starts_with("columns:") {
            in_columns = true;
            continue;
        }
        // Any other top-level key ends the block.
        if in_columns && !line.starts_with(' ') && !line.trim().is_empty() {
            break;
        }
        if in_columns && let Some(name) = line.strip_prefix("  - name: ") {
            names.push(name.trim().to_string());
        }
    }
    names
}

#[test]
fn every_column_profile_field_is_classified_as_value_bearing_or_derived() {
    let columns = entity_columns("column_profile");
    assert!(
        columns.len() > 20,
        "the YAML scan found only {} columns, which means the embedded format \
         changed and this gate is no longer reading the entity: {columns:?}",
        columns.len()
    );

    for column in &columns {
        let gated = VALUE_BEARING_COLUMN_FIELDS.contains(&column.as_str());
        let derived = DERIVED_COLUMN_FIELDS.contains(&column.as_str());
        assert!(
            gated || derived,
            "column_profile gained `{column}` and nothing says whether it echoes \
             a stored value. Add it to VALUE_BEARING_COLUMN_FIELDS if a reader \
             could recover a datum from it (a verbatim value, an extreme, a \
             frequent value), or to DERIVED_COLUMN_FIELDS if it is a count, a \
             length, or a statistic over many rows. The test asks because the \
             wire does not: an unclassified field reaches every viewer and \
             nothing fails."
        );
        assert!(
            !(gated && derived),
            "`{column}` is in both lists; it is one or the other"
        );
    }

    // The reverse direction: a list naming a column the entity no longer has is
    // dead weight that reads as coverage.
    for listed in VALUE_BEARING_COLUMN_FIELDS
        .iter()
        .chain(DERIVED_COLUMN_FIELDS.iter())
    {
        assert!(
            columns.iter().any(|c| c == listed),
            "`{listed}` is classified but column_profile has no such column — \
             remove it rather than leaving the list looking complete"
        );
    }
}

#[test]
fn the_five_known_value_bearing_fields_are_the_ones_gated() {
    // Spelled out so that *removing* a field from the gate is a visible edit to
    // a test that says why it was there, not a one-character deletion in a
    // const. Each of these is verbatim corpus data.
    let mut gated = VALUE_BEARING_COLUMN_FIELDS.to_vec();
    gated.sort_unstable();
    assert_eq!(
        gated,
        ["lex_max", "lex_min", "num_max", "num_min", "top_values"],
        "the set of fields that echo a stored value changed"
    );
}
