//! The ingest stage (port of `src/er/ingest/*`, DesignDoc.md S4.1).
//!
//! - [`landing`]: landing a delivery into `raw_records` and the S4.1 `content_hash`
//!   preimage. Incremental anti-join/classify/tombstone machinery lands later; a
//!   base delivery (no prior rows) is a plain insert.

pub mod landing;
