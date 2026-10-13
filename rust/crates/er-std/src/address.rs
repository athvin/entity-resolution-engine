//! The S4.2 `address_parse` v1 grammar (port of `src/er/std/address.py`).
//!
//! `RegexV1Parser` componentises one `address_line` into `addr_number`,
//! `addr_street`, `addr_unit` — all NULL together when the line is unrecognised
//! (S4.2 never guesses a partial parse). This is a second implementation of the
//! grammar `dbt/macros/std/address_parse.sql` runs; the three patterns are valid
//! in both RE2 (DuckDB) and Rust's `regex`, and the Python `RegexV1Parser` is the
//! shared oracle both are checked against.
//!
//! Whitespace handling is deliberately ASCII-only (`trim` removes spaces, not
//! U+00A0; the whitespace class is ASCII): a line carrying a non-breaking space is
//! left with it, exactly as DuckDB's `trim`/RE2 would, so the two implementations
//! cannot disagree on an input the wider Unicode forms would have split.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// The trailing unit phrase: `<keyword> <designator>` or `#<designator>`. Group 2
/// is the unit; the leading `(^| )` is part of the match so removing the phrase
/// takes its separator with it.
pub const ADDRESS_UNIT_PATTERN: &str =
    "(^| )((apt|apartment|unit|ste|suite|fl|floor|rm|room) [a-z0-9-]+|#[a-z0-9-]+)$";

/// A PO box in every spelling the punctuation strip leaves. Rejected, not parsed:
/// a box has no number/street/unit decomposition.
pub const ADDRESS_PO_BOX_PATTERN: &str = "^(p ?o|post office) box( .*)?$";

/// The house number: digits and at most one letter, then a boundary. `221b` is a
/// number; `5th` is not, because the boundary after the optional letter fails.
pub const ADDRESS_NUMBER_PATTERN: &str = "^([0-9]+[a-z]?)( |$)";

/// The S4.2 sentinel vocabulary (the oracle's spelling of
/// `null_semantics.sql::NULL_SENTINELS`).
pub const NULL_SENTINELS: [&str; 5] = ["", "null", "n/a", "-", "unknown"];

/// The `versions.address_parser_version` value that selects [`RegexV1Parser`].
pub const REGEX_V1_VERSION: &str = "1";

static UNIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(ADDRESS_UNIT_PATTERN).unwrap());
static PO_BOX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(ADDRESS_PO_BOX_PATTERN).unwrap());
static NUMBER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(ADDRESS_NUMBER_PATTERN).unwrap());
// DuckDB `trim` removes spaces only; RE2 `[[:space:]]` is ASCII. Both are spelled
// out rather than reached for through `.trim()` / `\s`, which are wider in Rust and
// would make the oracle disagree with the macro on an input carrying U+00A0.
static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("[ \t\n\x0B\x0C\r]+").unwrap());
static PUNCTUATION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("[.,]").unwrap());

const SPACE: char = ' ';

/// The three components S4.2 parses out of the single `address_line`. All three
/// are NULL together or the parse succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressComponents {
    pub addr_number: Option<String>,
    pub addr_street: Option<String>,
    pub addr_unit: Option<String>,
}

impl AddressComponents {
    /// What an unrecognised line parses to (all three NULL) — a stated outcome of
    /// the grammar (S4.2), not an incidental empty result.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            addr_number: None,
            addr_street: None,
            addr_unit: None,
        }
    }
}

/// The v1 grammar. Stateless and shareable.
#[derive(Debug, Clone, Copy, Default)]
pub struct RegexV1Parser;

impl RegexV1Parser {
    #[must_use]
    pub fn version(self) -> &'static str {
        REGEX_V1_VERSION
    }

    /// Return the components of `address_line`; all NULL if unrecognised.
    #[must_use]
    pub fn parse(self, address_line: Option<&str>) -> AddressComponents {
        let Some(line) = normalize(address_line) else {
            return AddressComponents::none();
        };

        let unit = UNIT_RE
            .captures(&line)
            .and_then(|caps| caps.get(2))
            .map(|m| m.as_str().to_string());
        // Unconditional, like the macro's `regexp_replace`: a line with no unit
        // phrase is left alone by the substitution rather than by a branch.
        let rest = UNIT_RE.replacen(&line, 1, "");
        let rest = rest.trim_matches(SPACE);

        let number = NUMBER_RE
            .captures(rest)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string());
        let street = NUMBER_RE.replacen(rest, 1, "");
        let street = street.trim_matches(SPACE);
        if street.is_empty() {
            // No street: a bare number, or a unit with nothing to attach it to.
            return AddressComponents::none();
        }
        AddressComponents {
            addr_number: number,
            addr_street: Some(street.to_string()),
            addr_unit: unit,
        }
    }
}

/// Apply the macro's normalization chain, in the macro's order:
/// `lowercase_trim` -> punctuation -> whitespace -> `null_semantics` -> PO box.
/// `null_semantics` runs LAST so the punctuation strip cannot manufacture a
/// sentinel out of a value the first pass let through.
fn normalize(address_line: Option<&str>) -> Option<String> {
    let address_line = address_line?;
    // `lowercase_trim`: NFC, trim, lower, NFC again — lowering can compose a
    // sequence that was not composable before, so one leading NFC is not enough.
    let first: String = address_line.nfc().collect();
    let text: String = first.trim_matches(SPACE).to_lowercase().nfc().collect();
    if text.is_empty() {
        return None;
    }

    let text = PUNCTUATION_RE.replace_all(&text, "");
    let text = WHITESPACE_RE.replace_all(&text, " ");
    let text = text.trim_matches(SPACE);
    if text.is_empty() || NULL_SENTINELS.contains(&text) {
        return None;
    }
    if PO_BOX_RE.is_match(text) {
        return None;
    }
    Some(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_one() {
        assert_eq!(RegexV1Parser.version(), "1");
    }

    #[test]
    fn grammar_branches() {
        let p = RegexV1Parser;
        assert_eq!(p.parse(None), AddressComponents::none());
        assert_eq!(p.parse(Some("PO Box 7")), AddressComponents::none());
        assert_eq!(p.parse(Some("123")), AddressComponents::none()); // bare number
        assert_eq!(p.parse(Some("Apt 5")), AddressComponents::none()); // unit, no street
        let c = p.parse(Some("123 Main St Apt 4"));
        assert_eq!(c.addr_number.as_deref(), Some("123"));
        assert_eq!(c.addr_street.as_deref(), Some("main st"));
        assert_eq!(c.addr_unit.as_deref(), Some("apt 4"));
        // 221b is a number; 5th is not.
        assert_eq!(
            p.parse(Some("221B Baker St")).addr_number.as_deref(),
            Some("221b")
        );
        assert_eq!(p.parse(Some("5th Avenue")).addr_number, None);
    }
}
