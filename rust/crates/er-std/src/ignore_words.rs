//! The ignore-words dictionary's pure core — the spec's second dictionary
//! type (stop words), sibling to [`crate::lexicon`]'s synonym pairs.
//!
//! A row is `(word, dictionary)`: the token to drop and the named dictionary
//! that drops it, so a tenant's `legal_suffixes` and `name_titles` evolve
//! independently. Like the lexicon, the dictionary is a matching input no
//! `content_hash` covers, so it joins the run fingerprint through
//! [`ignore_words_hash`] — a corpus standardized under one dictionary set
//! cannot be incrementally extended under another.
//!
//! Canonicalisation is DIRECTED, unlike the lexicon's: `(word, dictionary)`
//! means the word in that dictionary, so the pair is never orientation-swapped
//! the way a symmetric synonym pair is. Both halves are lowercased and
//! trimmed — matching is case-insensitive — before dedupe and hashing, so
//! `Inc,legal` and `inc , legal` are the same semantic row and hash
//! identically.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::lexicon::{RECORD_SEP, UNIT_SEP};

/// An ignore-words input refused at write time (HTTP 422 from the control
/// plane, exit `2` from the CLI): an empty word or dictionary name.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IgnoreWordsError {
    #[error("ignore-words row: {field} must be non-empty")]
    EmptyField { field: &'static str },
}

/// The canonical stored form of one half of a row: trimmed and lowercased.
/// `field` names the half in the refusal (`"word"` / `"dictionary"`).
pub fn normalize_field(value: &str, field: &'static str) -> Result<String, IgnoreWordsError> {
    let normalized = value.trim().to_lowercase();
    if normalized.is_empty() {
        return Err(IgnoreWordsError::EmptyField { field });
    }
    Ok(normalized)
}

/// The canonical row: both halves normalized. Ordering is preserved — this is
/// a directed pair.
pub fn canonical_row(word: &str, dictionary: &str) -> Result<(String, String), IgnoreWordsError> {
    Ok((
        normalize_field(word, "word")?,
        normalize_field(dictionary, "dictionary")?,
    ))
}

/// SHA-256 hex digest of the canonical active row set. Rows are deduped and
/// totally ordered as `(word, dictionary)` tuples, joined with the same
/// record/unit separators [`crate::lexicon::lexicon_hash`] uses — no
/// normalized word or dictionary name contains either control char. An empty
/// set hashes to the digest of the empty string: absence of rows is a state,
/// distinct from the relation never having been seeded (which the caller
/// reports as `None`).
#[must_use]
pub fn ignore_words_hash<I, S>(rows: I) -> String
where
    I: IntoIterator<Item = (S, S)>,
    S: AsRef<str>,
{
    let canonical: BTreeSet<(String, String)> = rows
        .into_iter()
        .map(|(w, d)| (w.as_ref().to_string(), d.as_ref().to_string()))
        .collect();
    let joined = canonical
        .iter()
        .map(|(w, d)| format!("{w}{UNIT_SEP}{d}"))
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
    fn canonicalises_case_and_whitespace() {
        assert_eq!(
            canonical_row(" Inc ", "Legal").unwrap(),
            ("inc".to_string(), "legal".to_string())
        );
        assert!(matches!(
            canonical_row("", "legal"),
            Err(IgnoreWordsError::EmptyField { field: "word" })
        ));
        assert!(matches!(
            canonical_row("inc", "  "),
            Err(IgnoreWordsError::EmptyField {
                field: "dictionary"
            })
        ));
    }

    #[test]
    fn the_pair_is_directed() {
        // (inc, legal) and (legal, inc) are different rows — no orientation
        // collapse the way the symmetric synonym lexicon has.
        assert_ne!(
            ignore_words_hash([("inc", "legal")]),
            ignore_words_hash([("legal", "inc")])
        );
    }

    #[test]
    fn dedupes_and_orders() {
        assert_eq!(
            ignore_words_hash([("inc", "legal"), ("inc", "legal"), ("llc", "legal")]),
            ignore_words_hash([("llc", "legal"), ("inc", "legal")])
        );
    }

    #[test]
    fn empty_set_is_sha256_of_empty() {
        assert_eq!(
            ignore_words_hash(Vec::<(String, String)>::new()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
