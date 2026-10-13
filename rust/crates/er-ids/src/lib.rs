//! Identity primitives — record keys, pair canonicalisation, ULID minting,
//! redirect resolution (Rust port of `src/er/entities/ids.py`). Depends on
//! nothing else in the engine (S5.0, D6/D9/D10).
//!
//! Note vs Python: the runtime-only `MonotonicUlidFactory` (the strict-monotonic
//! stream for `entity_events.event_id`) is deferred to the entities wave — it is
//! runtime behaviour, not a parity-testable pure function.

use std::collections::HashMap;

use ulid::Ulid;

/// S5.0: `record_key := source_system || ':' || source_record_id`.
pub const RECORD_KEY_SEPARATOR: char = ':';
/// Canonical Crockford base32 ULID width; every id sorts as a fixed-width VARCHAR.
pub const ULID_LENGTH: usize = 26;
/// Redirect-chain bound so a corrupt catalog fails fast instead of spinning.
pub const DEFAULT_MAX_HOPS: usize = 64;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    #[error("source_system must not be empty")]
    EmptySourceSystem,
    #[error("source_record_id must not be empty")]
    EmptySourceRecordId,
    #[error("source_system must not contain ':': {0:?}")]
    SeparatorInSourceSystem(String),
    #[error("source_record_id must not contain ':': {0:?}")]
    SeparatorInSourceRecordId(String),
    #[error("record key has no ':' separator: {0:?}")]
    NoSeparator(String),
    #[error("record key has an empty source_system: {0:?}")]
    EmptySplitSystem(String),
    #[error("record key has an empty source_record_id: {0:?}")]
    EmptySplitRecordId(String),
    #[error("a pair must have two distinct record keys, got {0:?} twice")]
    PairSameKey(String),
    #[error("redirect does not terminate: {0}")]
    IdCycle(String),
}

/// The canonical scalar record identity (S5.0). `':'` is banned in BOTH
/// components (stricter than S5.0's text) so `split_record_key` round-trips.
pub fn record_key(source_system: &str, source_record_id: &str) -> Result<String, IdError> {
    if source_system.is_empty() {
        return Err(IdError::EmptySourceSystem);
    }
    if source_record_id.is_empty() {
        return Err(IdError::EmptySourceRecordId);
    }
    if source_system.contains(RECORD_KEY_SEPARATOR) {
        return Err(IdError::SeparatorInSourceSystem(source_system.to_string()));
    }
    if source_record_id.contains(RECORD_KEY_SEPARATOR) {
        return Err(IdError::SeparatorInSourceRecordId(
            source_record_id.to_string(),
        ));
    }
    Ok(format!(
        "{source_system}{RECORD_KEY_SEPARATOR}{source_record_id}"
    ))
}

/// Inverse of [`record_key`], splitting on the FIRST separator only.
pub fn split_record_key(key: &str) -> Result<(String, String), IdError> {
    match key.split_once(RECORD_KEY_SEPARATOR) {
        None => Err(IdError::NoSeparator(key.to_string())),
        Some(("", _)) => Err(IdError::EmptySplitSystem(key.to_string())),
        Some((_, "")) => Err(IdError::EmptySplitRecordId(key.to_string())),
        Some((ss, srid)) => Ok((ss.to_string(), srid.to_string())),
    }
}

/// THE canonicalisation helper of S5.0/D9: `(a, b)` with `a < b` lexically.
pub fn canonicalize_pair(a: &str, b: &str) -> Result<(String, String), IdError> {
    if a == b {
        return Err(IdError::PairSameKey(a.to_string()));
    }
    Ok(if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    })
}

/// Source of freshly minted 26-char ULID strings (injected into reconcile so it
/// is a pure function of inputs + mint sequence). `&self` + interior mutability
/// because reconcile calls it sequentially within one run.
pub trait IdFactory {
    fn new_id(&self) -> String;
}

/// Production factory: an independent (random) ULID per call.
#[derive(Default)]
pub struct UlidFactory;

impl IdFactory for UlidFactory {
    fn new_id(&self) -> String {
        Ulid::new().to_string()
    }
}

/// Deterministic factory for tests: the base32 rendering of an ascending integer,
/// a valid 26-char ULID that sorts exactly as production ids do (Crockford base32
/// is ASCII-ascending at fixed width, so byte order IS lexical order).
pub struct CountingIdFactory {
    next: std::cell::Cell<u128>,
}

impl CountingIdFactory {
    #[must_use]
    pub fn new(start: u128) -> Self {
        Self {
            next: std::cell::Cell::new(start),
        }
    }
}

impl IdFactory for CountingIdFactory {
    fn new_id(&self) -> String {
        let value = self.next.get();
        self.next.set(value + 1);
        Ulid(value).to_string()
    }
}

/// Follow `entities.merged_into` to the entity that absorbed `entity_id`
/// (EXTERNAL id resolution, S4.5.3). Self if no redirect; errors on a cycle or a
/// chain longer than `max_hops`.
pub fn resolve<S: std::hash::BuildHasher>(
    entity_id: &str,
    redirects: &HashMap<String, String, S>,
    max_hops: usize,
) -> Result<String, IdError> {
    let mut seen = vec![entity_id.to_string()];
    let mut current = entity_id.to_string();
    while let Some(nxt) = redirects.get(&current) {
        if seen.iter().any(|s| s == nxt) {
            let mut chain = seen.clone();
            chain.push(nxt.clone());
            return Err(IdError::IdCycle(format!(
                "redirect cycle in entities.merged_into: {}",
                chain.join(" -> ")
            )));
        }
        if seen.len() > max_hops {
            return Err(IdError::IdCycle(format!(
                "redirect chain from {entity_id:?} did not terminate within max_hops={max_hops} (reached {current:?})"
            )));
        }
        seen.push(nxt.clone());
        current = nxt.clone();
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_key_round_trips_and_guards() {
        assert_eq!(record_key("crm", "42").unwrap(), "crm:42");
        assert_eq!(
            split_record_key("crm:42").unwrap(),
            ("crm".to_string(), "42".to_string())
        );
        // split on the FIRST separator only.
        assert_eq!(
            split_record_key("crm:a:b").unwrap(),
            ("crm".to_string(), "a:b".to_string())
        );

        assert_eq!(record_key("", "x"), Err(IdError::EmptySourceSystem));
        assert_eq!(record_key("s", ""), Err(IdError::EmptySourceRecordId));
        assert!(matches!(
            record_key("a:b", "x"),
            Err(IdError::SeparatorInSourceSystem(_))
        ));
        assert!(matches!(
            record_key("s", "a:b"),
            Err(IdError::SeparatorInSourceRecordId(_))
        ));
        assert!(matches!(
            split_record_key("nope"),
            Err(IdError::NoSeparator(_))
        ));
        assert!(matches!(
            split_record_key(":x"),
            Err(IdError::EmptySplitSystem(_))
        ));
        assert!(matches!(
            split_record_key("s:"),
            Err(IdError::EmptySplitRecordId(_))
        ));
    }

    #[test]
    fn canonicalize_pair_orders_and_rejects_dupes() {
        assert_eq!(
            canonicalize_pair("b", "a").unwrap(),
            ("a".to_string(), "b".to_string())
        );
        assert_eq!(
            canonicalize_pair("a", "b").unwrap(),
            ("a".to_string(), "b".to_string())
        );
        assert!(matches!(
            canonicalize_pair("a", "a"),
            Err(IdError::PairSameKey(_))
        ));
    }

    // Parity: str(ULID.from_int(n)) — fixed-width Crockford base32, byte order == int order.
    #[test]
    fn counting_factory_renders_like_python_ulid_from_int() {
        let f = CountingIdFactory::new(0);
        let a = f.new_id();
        let b = f.new_id();
        assert_eq!(a, "00000000000000000000000000");
        assert_eq!(b, "00000000000000000000000001");
        assert_eq!(a.len(), ULID_LENGTH);
        assert_eq!(b.len(), ULID_LENGTH);
        assert!(a < b, "mint order must equal lexical order");
    }

    #[test]
    fn resolve_follows_chain_and_detects_cycles() {
        let mut r = HashMap::new();
        // No redirect -> self.
        assert_eq!(resolve("E1", &r, DEFAULT_MAX_HOPS).unwrap(), "E1");
        // A chain E1 -> E2 -> E3 resolves to the claimant.
        r.insert("E1".to_string(), "E2".to_string());
        r.insert("E2".to_string(), "E3".to_string());
        assert_eq!(resolve("E1", &r, DEFAULT_MAX_HOPS).unwrap(), "E3");
        // A cycle errors.
        r.insert("E3".to_string(), "E1".to_string());
        assert!(matches!(
            resolve("E1", &r, DEFAULT_MAX_HOPS),
            Err(IdError::IdCycle(_))
        ));
        // A long chain past max_hops errors even without a cycle.
        let mut long = HashMap::new();
        for i in 0..100u32 {
            long.insert(format!("n{i}"), format!("n{}", i + 1));
        }
        assert!(matches!(resolve("n0", &long, 64), Err(IdError::IdCycle(_))));
    }
}
