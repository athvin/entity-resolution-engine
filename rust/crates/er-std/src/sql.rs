//! The S4.2 standardization macros as DuckDB SQL generators (port of
//! `dbt/macros/std/*.sql`).
//!
//! Each macro is pure Jinja over DuckDB string functions (no parsing library —
//! S2.1), so the Rust port is a function that renders the same SQL expression,
//! executed through dedupe-core's session. DuckDB runs identical SQL, so the
//! output is byte-identical to the dbt pipeline; the rendered text need not match
//! the macro's whitespace, only its semantics. Region and other `standardization`
//! settings are resolved at render time (the caller passes them), exactly as the
//! macros resolve them from the `standardization` var.

/// S4.2 `lowercase_trim(col)`: NFC-normalize, trim, lower, NFC again; `''` -> NULL.
/// The trailing NFC is load-bearing (lowering can compose a sequence the leading
/// NFC could not), and `nullif(…, '')` makes "absent" spell exactly one way.
#[must_use]
pub fn lowercase_trim(col: &str) -> String {
    format!("nullif(nfc_normalize(lower(trim(nfc_normalize({col})))), '')")
}

/// The S4.2 sentinel vocabulary, lowercase, as a SQL list body (`null_semantics`
/// compares the lowered/trimmed value against it).
pub const NULL_SENTINELS_SQL: &str = "'', 'null', 'n/a', '-', 'unknown'";

/// S4.2 `null_semantics(col)`: map the sentinel vocabulary to NULL, pass anything
/// else through VERBATIM (presence only; `lowercase_trim` decides spelling).
#[must_use]
pub fn null_semantics(col: &str) -> String {
    format!(
        "case when list_contains([{NULL_SENTINELS_SQL}], lower(trim({col}))) then null else {col} end"
    )
}

/// S4.2 `phone_e164(col)`: the two projections S5 names on `int_std_records`,
/// `phone_e164` and `phone_valid`. `region` is the `standardization.
/// phone_default_region` value (ISO alpha-2), resolved here and uppercased; v1
/// renders the NANP only (`US`), yielding NULL for any other region rather than a
/// guessed rendering. A value already carrying `+` is treated as E.164 and never
/// re-prefixed.
#[must_use]
pub fn phone_e164(col: &str, region: &str) -> String {
    let region = region.to_uppercase();
    let present = null_semantics(&lowercase_trim(col));
    let digits = format!("regexp_replace({present}, '[^0-9]', '', 'g')");
    let is_e164 = format!("starts_with({present}, '+')");
    let e164_ok = format!("length({digits}) between 7 and 15 and not starts_with({digits}, '0')");
    let region_branches = if region == "US" {
        format!(
            " when length({digits}) = 10 then '+1' || {digits} \
             when length({digits}) = 11 and starts_with({digits}, '1') then '+' || {digits}"
        )
    } else {
        String::new()
    };
    let rendered = format!(
        "case when {is_e164} then case when {e164_ok} then '+' || {digits} end{region_branches} end"
    );
    format!(
        "{rendered} as phone_e164, \
         case when {present} is null then null else {rendered} is not null end as phone_valid"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_is_uppercased_and_us_adds_nanp_branches() {
        let us = phone_e164("col", "us");
        assert!(us.contains("'+1' ||"), "US renders the NANP +1 branch");
        let gb = phone_e164("col", "GB");
        assert!(!gb.contains("'+1' ||"), "non-US renders no +1 branch");
    }

    #[test]
    fn helpers_render_expected_shape() {
        assert_eq!(
            lowercase_trim("x"),
            "nullif(nfc_normalize(lower(trim(nfc_normalize(x)))), '')"
        );
        assert!(null_semantics("x").contains("list_contains(['', 'null', 'n/a', '-', 'unknown']"));
    }
}
