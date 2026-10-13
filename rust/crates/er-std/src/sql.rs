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

/// S4.2 `email_norm(col)`: the two projections `email` and `email_valid`.
/// `strip_plus_addressing` and `placeholders` come from the `standardization` var
/// (S6). A configured placeholder nulls to `email=NULL, email_valid=NULL` (no
/// evidence), distinct from an unparsable value (`email=NULL, email_valid=false`).
/// The validity pattern is the S6.1 V16 placeholder pattern in RE2.
#[must_use]
pub fn email_norm(col: &str, strip_plus_addressing: bool, placeholders: &[&str]) -> String {
    let mut normalized = lowercase_trim(col);
    if strip_plus_addressing {
        // Only the first `+...` run of the local part, and only before the `@`.
        normalized = format!("regexp_replace({normalized}, '[+][^@]*@', '@')");
    }
    let valid = format!(
        "regexp_full_match({normalized}, '[^@[:space:]]+@[^@[:space:]]+[.][^@[:space:]]+')"
    );
    let is_placeholder = if placeholders.is_empty() {
        "false".to_string()
    } else {
        let literals = placeholders
            .iter()
            .map(|p| p.replace('\'', "''"))
            .collect::<Vec<_>>()
            .join("', '");
        format!("{normalized} in ('{literals}')")
    };
    format!(
        "case when {is_placeholder} or not {valid} then null else {normalized} end as email, \
         case when {normalized} is null or {is_placeholder} then null else {valid} end as email_valid"
    )
}

/// S4.2 `name_norm(col)`: a scalar expression applied to `given_name` /
/// `family_name`. `lowercase_trim`, then diacritic fold (re-composed to NFC),
/// then `-`/`_` → space, then strip `. ' ’ ,`, then collapse internal whitespace,
/// then `null_semantics` LAST (so the macro is unconditionally idempotent).
#[must_use]
pub fn name_norm(col: &str) -> String {
    let lowered = lowercase_trim(col);
    let folded = format!("nfc_normalize(strip_accents({lowered}))");
    let separators = format!("regexp_replace({folded}, '[-_]', ' ', 'g')");
    // `''` is one apostrophe to DuckDB; the class is `.` `'` `’` `,`.
    let punctuated = format!("regexp_replace({separators}, '[.''’,]', '', 'g')");
    let collapsed = format!("trim(regexp_replace({punctuated}, '[[:space:]]+', ' ', 'g'))");
    null_semantics(&format!("nullif({collapsed}, '')"))
}

/// The precision a `strptime` format can resolve to (S4.2): `day` if it carries a
/// day token (`%d`/`%-d`/`%j`), else `month` if a month token (`%m`/`%-m`/`%b`/`%B`),
/// else `year`. `%%` (a literal percent) is stripped first so it cannot be read as
/// a token prefix.
#[must_use]
pub fn date_precision(fmt: &str) -> &'static str {
    let format = fmt.replace("%%", "");
    const DAY: [&str; 3] = ["%d", "%-d", "%j"];
    const MONTH: [&str; 4] = ["%m", "%-m", "%b", "%B"];
    if DAY.iter().any(|t| format.contains(t)) {
        "day"
    } else if MONTH.iter().any(|t| format.contains(t)) {
        "month"
    } else {
        "year"
    }
}

/// S4.2 `parse_date(col, fmt)`: the scalar `birth_date DATE`. `fmt` is the
/// source's `sources.<name>.date_format` (S6), resolved at render time. A format
/// that cannot express a day renders a typed NULL (no `try_strptime` at all, so a
/// `%Y` source cannot fabricate January 1st); otherwise `try_strptime` refuses a
/// value the format does not fully describe. `null_semantics` runs first so a
/// sentinel is absence, not a parse failure.
#[must_use]
pub fn parse_date(col: &str, fmt: &str) -> String {
    if date_precision(fmt) != "day" {
        return "cast(null as date)".to_string();
    }
    let present = null_semantics(&lowercase_trim(col));
    let escaped = fmt.replace('\'', "''");
    format!("cast(try_strptime({present}, '{escaped}') as date)")
}

/// S4.2 `source_metadata(payload, spec)`: preserve the unmapped source fields of
/// `payload` (a JSON column) verbatim as a JSON object, keys sorted for
/// deterministic repeated deliveries. `excluded` is the set of keys NOT kept — the
/// caller builds it as `[record_id_column, updated_at_column, "persona_id"]` plus
/// every mapped `columns[attr]`. JSON pointers keep punctuation in field names
/// literal (`~`->`~0`, `/`->`~1`).
#[must_use]
pub fn source_metadata(payload: &str, excluded: &[&str]) -> String {
    let literals = excluded
        .iter()
        .map(|e| e.replace('\'', "''"))
        .collect::<Vec<_>>()
        .join("', '");
    format!(
        "to_json(map_from_entries(list_transform(\
         list_sort(list_filter(coalesce(json_keys({payload}), []::varchar[]), \
         k -> not list_contains(['{literals}'], k))), \
         k -> struct_pack(key := k, value := json_extract({payload}, \
         '/' || replace(replace(k, '~', '~0'), '/', '~1'))))))"
    )
}

/// S4.2 `name_variants(col)`: the `LIST(VARCHAR) NOT NULL` column. Never SQL NULL
/// — the EMPTY list when `name_norm(col)` is NULL, else a list whose element 0 is
/// exactly `name_norm(col)` (the symmetry guarantee, built by PREPENDING rather
/// than union+sort) followed by the sorted, de-duplicated one-hop lexicon
/// neighbours (symmetric read of the `active` rows; never a transitive closure).
/// `lexicon` is the qualified `nickname_variants` relation name.
#[must_use]
pub fn name_variants(col: &str, lexicon: &str) -> String {
    let normalized = name_norm(col);
    format!(
        "case when {normalized} is null then cast([] as varchar[]) \
         else list_prepend({normalized}, coalesce(( \
         select list_sort(list_distinct(list(edge.variant_other))) \
         from (select variant_a as variant_self, variant_b as variant_other from {lexicon} where active \
         union all \
         select variant_b as variant_self, variant_a as variant_other from {lexicon} where active) as edge \
         where edge.variant_self = {normalized}), cast([] as varchar[]))) end"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_precision_tokens() {
        assert_eq!(date_precision("%Y-%m-%d"), "day");
        assert_eq!(date_precision("%m/%d/%Y"), "day");
        assert_eq!(date_precision("%Y-%m"), "month");
        assert_eq!(date_precision("%Y"), "year");
        assert_eq!(parse_date("col", "%Y"), "cast(null as date)");
    }

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
