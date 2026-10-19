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
    #[error("not a lake YYYY-MM-DD HH:MM:SS timestamp: {0:?}")]
    BadTimestamp(String),
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

/// Canonical lowercase-hyphenated UUID text width; every v7 id sorts as a
/// fixed-width VARCHAR (the hex alphabet is ASCII-ascending, so byte order on
/// the canonical text IS timestamp-then-counter order).
pub const UUID_LENGTH: usize = 36;

/// Mint a wall-clock UUIDv7 as canonical lowercase text — the identity
/// convention for one-shot application mints (org/job/api_key/schedule/
/// webhook/staged_action ids). Time-ordered, so fresh ids cluster in indexes.
#[must_use]
pub fn new_v7() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Mint a *deterministic* UUIDv7 from a logical timestamp and a mint counter —
/// the rerun-reproducible variant for reconcile's entity ids: the run's logical
/// time fills the 48-bit timestamp field and the counter fills `rand_b`, so the
/// same inputs always mint the same id and mint order equals lexical order.
/// The counter must fit the 62-bit `rand_b` field.
#[must_use]
pub fn deterministic_v7(unix_ts_ms: u64, counter: u64) -> String {
    assert!(
        counter < (1 << 62),
        "deterministic_v7 counter must fit UUIDv7's 62-bit rand_b field"
    );
    let mut rand = [0u8; 10];
    rand[2..].copy_from_slice(&counter.to_be_bytes());
    uuid::Builder::from_unix_timestamp_millis(unix_ts_ms, &rand)
        .into_uuid()
        .to_string()
}

/// The deterministic UUIDv7 mint stream for reconcile's entity ids: every
/// [`IdFactory::new_id`] call is [`deterministic_v7`] of the run's logical
/// timestamp and an ascending counter — a pure function of (run clock, mint
/// sequence), so reruns mint byte-identical ids and mint order is lexical
/// order. The counter starts past every id ever minted, like the retired
/// counting-ULID stream.
pub struct DeterministicV7Factory {
    unix_ts_ms: u64,
    next: std::cell::Cell<u64>,
}

impl DeterministicV7Factory {
    #[must_use]
    pub fn new(unix_ts_ms: u64, start: u64) -> Self {
        Self {
            unix_ts_ms,
            next: std::cell::Cell::new(start),
        }
    }

    /// Build from the lake's `YYYY-MM-DD HH:MM:SS` timestamp literal (the
    /// run-logical clock every stage already threads). Errors on any other
    /// shape — a malformed run stamp must fail loudly, not mint from zero.
    pub fn from_lake_timestamp(ts: &str, start: u64) -> Result<Self, IdError> {
        let ms =
            parse_lake_timestamp_ms(ts).ok_or_else(|| IdError::BadTimestamp(ts.to_string()))?;
        Ok(Self::new(ms, start))
    }
}

impl IdFactory for DeterministicV7Factory {
    fn new_id(&self) -> String {
        let value = self.next.get();
        self.next.set(value + 1);
        deterministic_v7(self.unix_ts_ms, value)
    }
}

/// Parse the lake's `YYYY-MM-DD HH:MM:SS` literal to Unix epoch milliseconds
/// (UTC; the inverse of the dispatcher's civil-from-days rendering).
#[must_use]
pub fn parse_lake_timestamp_ms(ts: &str) -> Option<u64> {
    let (date, time) = ts.split_once(' ')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let mut c = time.split(':');
    let (hh, mm, ss): (i64, i64, i64) = (
        c.next()?.parse().ok()?,
        c.next()?.parse().ok()?,
        c.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    // Howard Hinnant's days-from-civil (exact, dependency-free).
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    u64::try_from(secs).ok()?.checked_mul(1000)
}

/// The content-derived id for SQL/CTAS-minted rows: `sha256('<relation>:' ||
/// k1 || ':' || …)` as 64 lowercase hex chars — byte-identical to DuckDB's
/// `sha256(...)` over the same concatenation, so a Rust mint and a SQL mint of
/// the same natural key agree. The relation prefix domain-separates relations;
/// parts are the already-canonical natural-key values, joined with `':'`
/// exactly as the SQL recipe writes them.
#[must_use]
pub fn natural_id(relation: &str, parts: &[&str]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(relation.as_bytes());
    for part in parts {
        hasher.update(b":");
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
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
    fn new_v7_mints_canonical_version_7_text() {
        let id = new_v7();
        assert_eq!(id.len(), UUID_LENGTH);
        assert_eq!(&id[14..15], "7", "version nibble must be 7: {id}");
        assert!(
            matches!(&id[19..20], "8" | "9" | "a" | "b"),
            "RFC variant expected: {id}"
        );
        assert_eq!(id, id.to_lowercase(), "canonical text is lowercase");
    }

    // The determinism contract reconcile's entity ids rely on: same logical
    // inputs -> the same id, and mint order == lexical order.
    #[test]
    fn deterministic_v7_is_stable_and_sorts_by_mint_order() {
        let a1 = deterministic_v7(1_700_000_000_000, 0);
        let a2 = deterministic_v7(1_700_000_000_000, 0);
        assert_eq!(a1, a2, "reruns must mint byte-identical ids");
        assert_eq!(a1.len(), UUID_LENGTH);
        assert_eq!(&a1[14..15], "7");

        let b = deterministic_v7(1_700_000_000_000, 1);
        let c = deterministic_v7(1_700_000_000_001, 0);
        assert!(a1 < b, "same tick: counter order is lexical order");
        assert!(b < c, "a later logical tick sorts after every earlier mint");
    }

    // Must stay byte-identical to DuckDB's sha256('<relation>:'||k1||':'||k2):
    // the SQL mints and the Rust mints of the same natural key must agree.
    #[test]
    fn natural_id_matches_the_sql_recipe() {
        assert_eq!(
            natural_id("review_queue", &["crm:1", "crm:2"]),
            sha256_hex("review_queue:crm:1:crm:2")
        );
        assert_eq!(natural_id("t", &[]), sha256_hex("t"));
        let id = natural_id("memberships", &["e1", "r1"]);
        assert_eq!(id.len(), 64);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    fn sha256_hex(text: &str) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(text.as_bytes());
        digest.iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        })
    }

    #[test]
    fn deterministic_v7_factory_is_a_pure_function_of_clock_and_sequence() {
        let f1 = DeterministicV7Factory::from_lake_timestamp("2026-01-01 00:00:00", 7).unwrap();
        let f2 = DeterministicV7Factory::from_lake_timestamp("2026-01-01 00:00:00", 7).unwrap();
        let (a1, b1) = (f1.new_id(), f1.new_id());
        assert_eq!(a1, f2.new_id(), "rerun mints the same stream");
        assert!(a1 < b1, "mint order is lexical order");
        assert_eq!(&a1[14..15], "7");
        assert!(DeterministicV7Factory::from_lake_timestamp("yesterday", 0).is_err());
    }

    #[test]
    fn parse_lake_timestamp_matches_known_epochs() {
        assert_eq!(parse_lake_timestamp_ms("1970-01-01 00:00:00"), Some(0));
        assert_eq!(
            parse_lake_timestamp_ms("2026-01-01 00:00:00"),
            Some(1_767_225_600_000)
        );
        assert_eq!(parse_lake_timestamp_ms("2026-13-01 00:00:00"), None);
        assert_eq!(parse_lake_timestamp_ms("nope"), None);
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
