//! S3 auto-ingestion (E9c): a dropped object becomes a delivery.
//!
//! A tenant points a prefix at us, drops a CSV, and the dispatcher picks it up
//! on its next tick. No webhook to configure, no push credential to issue.
//!
//! # Why it downloads rather than teaching the engine to read S3
//!
//! `run_job` reads `<drop_root>/<source>.csv` from the local filesystem, and
//! `delivery_preconditions` / `finish_delivery` are the single landing path
//! both the CSV upload and the JSON push already go through. Downloading into
//! `drop_root` keeps that true: S3 becomes another way a file ARRIVES, not a
//! second way it is ingested. Teaching the engine to read object storage would
//! have duplicated the landing checks, which is where every "did my file do
//! anything" answer comes from.
//!
//! # The `.part` rename
//!
//! An object is downloaded to `<name>.part` and renamed into place, because a
//! dispatcher tick that lands mid-download would otherwise read a truncated
//! CSV and report a successful delivery of half a file. Rename within a
//! filesystem is atomic; a direct write is not. This is the same discipline
//! W0.4's upload landing path uses, and the reason is identical.

use std::path::{Path, PathBuf};

/// One object worth ingesting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drop {
    /// The S3 key, kept whole so the ledger can say exactly what was read.
    pub key: String,
    /// The source this maps onto — the first path segment after the prefix.
    pub source: String,
    pub size: i64,
}

/// Why an object was passed over. Carried rather than dropped so a tenant
/// asking "why was my file ignored" gets an answer instead of silence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// A prefix placeholder or a directory marker, not a file.
    NotAFile(String),
    /// Not a CSV. Other formats are a real request, but accepting one here
    /// silently would ingest it as CSV and produce garbage rows.
    WrongExtension(String),
    /// Empty. A zero-byte object is almost always an interrupted upload, and
    /// ingesting it would report "0 new records" as if the file said so.
    Empty(String),
    /// Larger than the delivery cap.
    TooLarge { key: String, size: i64 },
    /// The path has no source segment, so there is nothing to map it onto.
    NoSource(String),
}

/// The largest object the poller will download.
///
/// Mirrors the upload path's own cap rather than inventing a second number:
/// a file too big to POST is too big to arrive this way, and a tenant should
/// not discover that the two paths disagree.
pub const MAX_OBJECT_BYTES: i64 = 256 * 1024 * 1024;

/// Classify one listed object against a prefix.
///
/// `prefix` is the tenant's configured root; everything under it is expected
/// to be `<prefix>/<source>/<file>.csv`. The source comes from the PATH rather
/// than from the filename, because a tenant dropping `2026-01-05.csv` into
/// `crm/` means the crm source — making them encode the source in every
/// filename would be a rule they discover by having a file ignored.
pub fn classify(key: &str, size: i64, prefix: &str) -> Result<Drop, Skipped> {
    if key.ends_with('/') {
        return Err(Skipped::NotAFile(key.to_string()));
    }
    let rest = key.strip_prefix(prefix).unwrap_or(key);
    let rest = rest.trim_start_matches('/');

    let mut segments = rest.split('/');
    let source = segments.next().unwrap_or_default();
    let file = segments.next_back().unwrap_or_default();
    // A bare `<prefix>/file.csv` has no source segment: `source` and `file`
    // would be the same string, which names no source.
    if source.is_empty() || file.is_empty() || source == file {
        return Err(Skipped::NoSource(key.to_string()));
    }
    if !file.to_ascii_lowercase().ends_with(".csv") {
        return Err(Skipped::WrongExtension(key.to_string()));
    }
    if size <= 0 {
        return Err(Skipped::Empty(key.to_string()));
    }
    if size > MAX_OBJECT_BYTES {
        return Err(Skipped::TooLarge {
            key: key.to_string(),
            size,
        });
    }
    Ok(Drop {
        key: key.to_string(),
        source: source.to_string(),
        size,
    })
}

/// Where the bytes are written before `finish_delivery` renames them into
/// place.
///
/// The name matches the upload path's own — `.{source}.{delivery_id}.csv.part`
/// — rather than inventing a second convention. Two things depend on it: the
/// leading dot keeps a half-written file from looking like a source, and the
/// delivery id keeps two concurrent deliveries of one source from writing over
/// each other. `finish_delivery` derives the final name from this path's
/// directory, so getting the shape right is what makes the rename land.
///
/// Returns `None` when the source name would escape `drop_root` — a key like
/// `../../etc/crm/x.csv` classifies with source `..`, and joining that
/// unchecked writes outside the tenant's directory. This is the second of two
/// checks (`delivery_preconditions` validates the source as a path segment
/// first) and it is kept because the listing comes from S3, which a tenant
/// controls: a guard at the join itself cannot be bypassed by a future caller
/// that forgets the first one.
pub fn staging_path(drop_root: &Path, source: &str, delivery_id: &str) -> Option<PathBuf> {
    if source.is_empty()
        || source.contains('/')
        || source.contains('\\')
        || source == "."
        || source == ".."
    {
        return None;
    }
    Some(drop_root.join(format!(".{source}.{delivery_id}.csv.part")))
}

/// Whether a listed object is one this source has not already ingested.
///
/// Compares BOTH the key and the ETag against the cursor. The key alone is not
/// enough: a tenant who overwrites `crm/daily.csv` every night leaves the key
/// identical, and a key-only cursor would read the first night's file and then
/// never look again — the failure would present as "our nightly export stopped
/// working" with nothing in any log, because from the poller's side nothing
/// happened at all.
pub fn is_new(drop: &Drop, etag: Option<&str>, cursor: Option<(&str, Option<&str>)>) -> bool {
    match cursor {
        None => true,
        Some((seen_key, seen_etag)) => drop.key != seen_key || etag != seen_etag,
    }
}

/// One listed object: the drop, when it was last modified, and its ETag.
pub type Listed = (Drop, i64, Option<String>);

/// The one object a source should ingest out of everything under its prefix.
///
/// A tenant dropping a file a night leaves a prefix full of them, and ingesting
/// every one in a single tick would replay their history as a burst of runs.
/// Only the newest is taken; the rest stay in the bucket, which is the tenant's
/// archive rather than ours to delete.
///
/// **Ties break on the key, lexicographically highest.** S3 last-modified
/// timestamps have one-second granularity, so two files written in the same
/// second tie — which happens exactly when a tenant uploads a batch. Without a
/// tie-break the chosen object would depend on listing order, and the same
/// prefix could ingest a different file on each tick without anything changing
/// in the bucket. Date-named files make the highest key the latest one, which
/// is also the useful answer.
#[must_use]
pub fn newest(listed: Vec<Listed>) -> Option<(Drop, Option<String>)> {
    listed
        .into_iter()
        .max_by(|(a, a_mod, _), (b, b_mod, _)| a_mod.cmp(b_mod).then_with(|| a.key.cmp(&b.key)))
        .map(|(drop, _, etag)| (drop, etag))
}

/// How many 1000-key listing pages the poller will read for one source.
///
/// Ten pages is ten thousand objects — a tenant dropping a file a night for
/// twenty-seven years. See `Watcher::newest` for why hitting this refuses the
/// source rather than working with what it has.
const MAX_LIST_PAGES: usize = 10;

/// Why a poll of one source produced nothing.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// The listing or download failed.
    #[error("reading s3://{bucket}/{prefix}")]
    S3 {
        bucket: String,
        prefix: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The prefix holds more objects than one poll will examine.
    ///
    /// This **refuses** rather than taking the newest of what it saw, and the
    /// reason is the listing order: S3 returns keys lexicographically, so a
    /// truncated listing of `2026-01-05.csv`-style names drops the *highest*
    /// keys — exactly the newest files. A cap that silently kept going would
    /// therefore ingest a stale file and report success, which is worse than
    /// ingesting nothing and saying so.
    #[error(
        "s3://{bucket}/{prefix} holds more than {examined} objects; the poller will not guess \
         which is newest from a truncated listing — prune the prefix or point the source at a \
         narrower one"
    )]
    PrefixTooLarge {
        bucket: String,
        prefix: String,
        examined: usize,
    },
    /// The object disappeared between listing and download, or the body failed
    /// mid-stream.
    #[error("downloading s3://{bucket}/{key}")]
    Download {
        bucket: String,
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// The S3 half: list a source's prefix, download the one object worth reading.
///
/// Separate from the pure functions above so the classification rules — which
/// are where the bugs live — stay testable without a bucket, a network or a
/// credential.
pub struct Watcher {
    client: aws_sdk_s3::Client,
}

impl Watcher {
    /// Build a watcher from the default credential chain.
    ///
    /// In-cluster that is the container-credentials provider EKS Pod Identity
    /// populates, which is why `infra/terraform/envs/dev/lake.tf`'s static
    /// `ER_S3_ACCESS_KEY_ID` pair does not need to be plumbed here: the same
    /// chain reads either, and the pod role is the one that should win.
    pub async fn from_env() -> Self {
        // An explicit ring-backed HTTPS client, for the reason mail.rs gives:
        // the SDK's default resolves TLS to aws-lc-rs, whose -sys crate needs a
        // cmake the build image lacks.
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
                aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
            ))
            .build_https();
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .http_client(http)
            .load()
            .await;
        Self {
            client: aws_sdk_s3::Client::new(&config),
        }
    }

    /// The newest object under `<prefix>/<source>/`, with its ETag.
    ///
    /// Returns `Ok(None)` when the prefix holds nothing worth reading, and
    /// reports every passed-over object through `skipped` so a tenant asking
    /// "why was my file ignored" gets an answer. Those skips are logged rather
    /// than returned because they are not failures — a `.json` sitting beside
    /// the CSVs is a normal thing for a bucket to contain.
    pub async fn newest(
        &self,
        bucket: &str,
        prefix: &str,
        source: &str,
        skipped: &mut Vec<Skipped>,
    ) -> Result<Option<(Drop, Option<String>)>, WatchError> {
        let source_prefix = format!("{}/{source}/", prefix.trim_end_matches('/'));
        let mut listed: Vec<Listed> = Vec::new();
        let mut token: Option<String> = None;
        let mut examined = 0usize;
        for _ in 0..MAX_LIST_PAGES {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(bucket)
                .prefix(&source_prefix);
            if let Some(t) = &token {
                req = req.continuation_token(t);
            }
            let page = req.send().await.map_err(|e| WatchError::S3 {
                bucket: bucket.to_string(),
                prefix: source_prefix.clone(),
                source: Box::new(e),
            })?;
            for object in page.contents() {
                let Some(key) = object.key() else { continue };
                examined += 1;
                match classify(key, object.size().unwrap_or(0), prefix) {
                    Ok(drop) => listed.push((
                        drop,
                        object
                            .last_modified()
                            .map_or(0, aws_sdk_s3::primitives::DateTime::secs),
                        object.e_tag().map(str::to_string),
                    )),
                    Err(why) => skipped.push(why),
                }
            }
            if page.is_truncated().unwrap_or(false) {
                token = page.next_continuation_token().map(str::to_string);
            } else {
                // The selection — including the tie-break — is `newest`'s, so
                // the rule that decides which of a tenant's files gets read is
                // testable without a bucket.
                return Ok(newest(listed));
            }
        }
        Err(WatchError::PrefixTooLarge {
            bucket: bucket.to_string(),
            prefix: source_prefix,
            examined,
        })
    }

    /// Download one object to `dest`, returning the bytes written.
    ///
    /// Streamed rather than buffered, for the reason the upload handler gives:
    /// the cap is 256 MiB and the dispatcher must not hold that in memory per
    /// poll. A failure mid-stream leaves `dest` behind, which the caller
    /// removes — a `.part` file nobody cleans up would accumulate silently.
    pub async fn download(&self, bucket: &str, key: &str, dest: &Path) -> Result<u64, WatchError> {
        let fail = |e: Box<dyn std::error::Error + Send + Sync>| WatchError::Download {
            bucket: bucket.to_string(),
            key: key.to_string(),
            source: e,
        };
        let mut object = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| fail(Box::new(e)))?;
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| fail(Box::new(e)))?;
        let mut wrote = 0u64;
        while let Some(chunk) = object
            .body
            .try_next()
            .await
            .map_err(|e| fail(Box::new(e)))?
        {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                .await
                .map_err(|e| fail(Box::new(e)))?;
            wrote += chunk.len() as u64;
        }
        tokio::io::AsyncWriteExt::flush(&mut file)
            .await
            .map_err(|e| fail(Box::new(e)))?;
        Ok(wrote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_a_normal_drop() {
        let drop = classify("tenants/acme/crm/2026-01-05.csv", 1_000, "tenants/acme").unwrap();
        assert_eq!(drop.source, "crm");
        assert_eq!(drop.key, "tenants/acme/crm/2026-01-05.csv");
    }

    #[test]
    fn takes_the_source_from_the_path_not_the_filename() {
        // A tenant dropping `2026-01-05.csv` into `crm/` means the crm source.
        // Requiring the source in every filename is a rule they would discover
        // by having a file silently ignored.
        let drop = classify("acme/billing/anything.csv", 10, "acme").unwrap();
        assert_eq!(drop.source, "billing");
    }

    #[test]
    fn skips_a_directory_marker() {
        assert_eq!(
            classify("acme/crm/", 0, "acme"),
            Err(Skipped::NotAFile("acme/crm/".into()))
        );
    }

    #[test]
    fn skips_a_file_with_no_source_segment() {
        // `<prefix>/file.csv` names no source, so there is nothing to map it
        // onto — ingesting it under a guessed source would attribute a
        // tenant's records to the wrong system.
        assert_eq!(
            classify("acme/loose.csv", 10, "acme"),
            Err(Skipped::NoSource("acme/loose.csv".into()))
        );
    }

    #[test]
    fn skips_a_non_csv() {
        // Accepting one silently would ingest it AS CSV and produce garbage
        // rows that look like real data.
        assert!(matches!(
            classify("acme/crm/data.parquet", 10, "acme"),
            Err(Skipped::WrongExtension(_))
        ));
        // Case-insensitively: S3 keys are case-sensitive and `.CSV` is common
        // from Windows exporters.
        assert!(classify("acme/crm/DATA.CSV", 10, "acme").is_ok());
    }

    #[test]
    fn skips_an_empty_object() {
        // Almost always an interrupted upload. Ingesting it would report
        // "0 new records" as though the file had said so.
        assert!(matches!(
            classify("acme/crm/x.csv", 0, "acme"),
            Err(Skipped::Empty(_))
        ));
    }

    #[test]
    fn skips_an_object_past_the_cap() {
        assert!(matches!(
            classify("acme/crm/x.csv", MAX_OBJECT_BYTES + 1, "acme"),
            Err(Skipped::TooLarge { .. })
        ));
        assert!(classify("acme/crm/x.csv", MAX_OBJECT_BYTES, "acme").is_ok());
    }

    #[test]
    fn refuses_a_source_that_would_escape_the_drop_root() {
        // THE one that matters: the listing comes from S3, which the tenant
        // controls, so `..` is reachable input and joining it unchecked writes
        // outside their directory.
        let root = Path::new("/var/drop/acme");
        assert!(staging_path(root, "..", "d1").is_none());
        assert!(staging_path(root, "../../etc", "d1").is_none());
        assert!(staging_path(root, "a/b", "d1").is_none());
        assert!(staging_path(root, "", "d1").is_none());
        assert!(staging_path(root, ".", "d1").is_none());
    }

    #[test]
    fn stages_under_the_upload_paths_own_naming() {
        // The shape is not cosmetic: `finish_delivery` derives the final name
        // from this path's directory, the leading dot keeps a half-written file
        // from looking like a source, and the delivery id keeps two concurrent
        // deliveries of one source off each other's bytes.
        let part = staging_path(Path::new("/var/drop/acme"), "crm", "d1").unwrap();
        assert_eq!(
            part,
            Path::new("/var/drop/acme/.crm.d1.csv.part"),
            "the staging name must match the upload path's, or the rename lands \
             somewhere nothing reads"
        );
        let other = staging_path(Path::new("/var/drop/acme"), "crm", "d2").unwrap();
        assert_ne!(part, other, "two deliveries of one source must not collide");
    }

    #[test]
    fn an_overwritten_key_counts_as_new() {
        // The failure this prevents presents as "our nightly export stopped
        // working", with nothing in any log: a tenant overwriting
        // `crm/daily.csv` leaves the key identical, so a key-only cursor reads
        // the first night's file and never looks again.
        let drop = Drop {
            key: "acme/crm/daily.csv".into(),
            source: "crm".into(),
            size: 10,
        };
        assert!(
            is_new(
                &drop,
                Some("etag-2"),
                Some(("acme/crm/daily.csv", Some("etag-1")))
            ),
            "same key, new content — this is the whole reason etag is stored"
        );
        assert!(
            !is_new(
                &drop,
                Some("etag-1"),
                Some(("acme/crm/daily.csv", Some("etag-1")))
            ),
            "unchanged: re-ingesting would re-run the tenant's pipeline every tick"
        );
        assert!(
            is_new(&drop, Some("etag-1"), None),
            "no cursor yet — the first poll must read something"
        );
        assert!(
            is_new(
                &drop,
                Some("etag-1"),
                Some(("acme/crm/day1.csv", Some("etag-1")))
            ),
            "a different key is new even when the bytes happen to match"
        );
    }

    #[test]
    fn keeps_only_the_newest() {
        // A tenant dropping a file a night leaves a prefix full of them;
        // ingesting every one in a tick would replay their history as a burst
        // of runs.
        let at = |key: &str, modified: i64| {
            (
                Drop {
                    key: key.into(),
                    source: "crm".into(),
                    size: 1,
                },
                modified,
                Some(format!("etag-{key}")),
            )
        };
        let (drop, etag) = newest(vec![
            at("acme/crm/day1.csv", 100),
            at("acme/crm/day3.csv", 300),
            at("acme/crm/day2.csv", 200),
        ])
        .expect("three objects, one newest");
        assert_eq!(drop.key, "acme/crm/day3.csv");
        assert_eq!(etag.as_deref(), Some("etag-acme/crm/day3.csv"));
    }

    #[test]
    fn breaks_a_timestamp_tie_on_the_key() {
        // S3 last-modified has one-second granularity, so a tenant uploading a
        // batch produces ties. Without a tie-break the chosen object depends on
        // listing order, and the same prefix could ingest a different file on
        // every tick with nothing having changed in the bucket.
        let at = |key: &str| {
            (
                Drop {
                    key: key.into(),
                    source: "crm".into(),
                    size: 1,
                },
                500,
                None,
            )
        };
        let forwards = newest(vec![at("acme/crm/a.csv"), at("acme/crm/b.csv")]).unwrap();
        let backwards = newest(vec![at("acme/crm/b.csv"), at("acme/crm/a.csv")]).unwrap();
        assert_eq!(
            forwards.0.key, backwards.0.key,
            "listing order must not decide"
        );
        assert_eq!(
            forwards.0.key, "acme/crm/b.csv",
            "highest key wins, which for date-named files is the latest"
        );
    }

    #[test]
    fn an_empty_prefix_selects_nothing() {
        // Not an error: a tenant who has configured a bucket and not yet
        // dropped a file is in a normal state, and reporting it as a failure
        // every minute would bury the real ones.
        assert!(newest(vec![]).is_none());
    }
}
