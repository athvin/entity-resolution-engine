//! The nickname lexicon's pure core (port of the pure parts of
//! `src/er/std/lexicon.py`, DesignDoc.md S4.2/S4.0/S5.1).
//!
//! The lexicon is a matching input no `content_hash` covers, so S4.0/S5.1 track
//! it with its own fingerprint, [`lexicon_hash`] — recorded in `runs.lexicon_hash`
//! and compared by the config-drift guard (see `er-versions`). A pair is
//! canonicalised ([`canonical_pair`]: names lowercased/trimmed, the two distinct,
//! stored as `min < max`) before it is written or hashed, so an orientation
//! duplicate — `(a, b)` beside `(b, a)` — is the same semantic lexicon and hashes
//! identically.
//!
//! The DuckDB-backed verbs (`add_pair`/`remove_pair`/`seed_lexicon`/…) are not
//! here: they need a lake connection and land with the standardization stage. This
//! module is the pure grammar the stage and the drift guard share.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

/// Separator between the two names of one pair (ASCII record separator). The same
/// control char `std_hash` uses for `name_variants` — no normalized name contains it.
pub const UNIT_SEP: char = '\u{1e}';
/// Separator between pairs (ASCII unit separator).
pub const RECORD_SEP: char = '\u{1f}';

/// A lexicon input S4.0 rejects (exit `2`): an empty name, or a pair whose two
/// names collapse to one value (the macro's one-hop walk would expand it into
/// nothing).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LexiconError {
    #[error("er lexicon: {flag} must be a non-empty name")]
    EmptyName { flag: String },
    #[error(
        "er lexicon: {name:?} cannot be its own variant; the pair must name two distinct normalized names"
    )]
    SelfVariant { name: String },
}

impl LexiconError {
    /// The S4.0 exit status a bad lexicon input gets.
    #[must_use]
    pub fn code(&self) -> i32 {
        2
    }
}

/// The lowercase, trimmed form the `name_norm` macro compares against, or the
/// exit-`2` refusal for an empty name. Whitespace is the full Unicode set
/// (Python `str.strip()`), matching the lexicon verbs rather than the ASCII-only
/// `address` grammar.
pub fn normalize_name(name: &str, flag: &str) -> Result<String, LexiconError> {
    let normalized = name.trim().to_lowercase();
    if normalized.is_empty() {
        return Err(LexiconError::EmptyName {
            flag: flag.to_string(),
        });
    }
    Ok(normalized)
}

/// The stored orientation of a pair: both names normalized, required distinct, and
/// returned as `(min, max)` lexically (S5.0's filtered-key shape).
pub fn canonical_pair(a: &str, b: &str) -> Result<(String, String), LexiconError> {
    let first = normalize_name(a, "--a")?;
    let second = normalize_name(b, "--b")?;
    if first == second {
        return Err(LexiconError::SelfVariant { name: first });
    }
    Ok(if first < second {
        (first, second)
    } else {
        (second, first)
    })
}

/// SHA-256 hex digest of the canonical active pair set (S5.1). `pairs` are the
/// already-normalized active rows; each is oriented `min < max`, the set is
/// deduped and totally ordered, then joined (`a␞b` within a pair, `␟` between) and
/// hashed. An empty set hashes to the digest of the empty string — absence of
/// pairs is a state, not the absence of the relation (which the DB layer reports
/// as `None`).
#[must_use]
pub fn lexicon_hash<I, S>(pairs: I) -> String
where
    I: IntoIterator<Item = (S, S)>,
    S: AsRef<str>,
{
    let oriented: BTreeSet<(String, String)> = pairs
        .into_iter()
        .map(|(a, b)| {
            let (a, b) = (a.as_ref().to_string(), b.as_ref().to_string());
            if a < b { (a, b) } else { (b, a) }
        })
        .collect();
    let joined = oriented
        .iter()
        .map(|(a, b)| format!("{a}{UNIT_SEP}{b}"))
        .collect::<Vec<_>>()
        .join(&RECORD_SEP.to_string());
    let mut hasher = Sha256::new();
    hasher.update(joined.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_orients_and_rejects() {
        assert_eq!(
            canonical_pair("Bob", " Robert ").unwrap(),
            ("bob".to_string(), "robert".to_string())
        );
        // min < max regardless of argument order
        assert_eq!(
            canonical_pair("tony", "Anthony").unwrap(),
            ("anthony".to_string(), "tony".to_string())
        );
        assert_eq!(
            canonical_pair("Al", "al"),
            Err(LexiconError::SelfVariant {
                name: "al".to_string()
            })
        );
        assert!(matches!(
            canonical_pair("", "x"),
            Err(LexiconError::EmptyName { .. })
        ));
    }

    #[test]
    fn empty_set_is_sha256_of_empty() {
        assert_eq!(
            lexicon_hash(Vec::<(String, String)>::new()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn orientation_duplicates_collapse() {
        let with_dup = lexicon_hash([("bob", "robert"), ("robert", "bob")]);
        let single = lexicon_hash([("bob", "robert")]);
        assert_eq!(with_dup, single);
    }
}
