//! Resumable bulk uploads (#1012): a file too large for one HTTP import arrives
//! as numbered parts, each a bounded request a client can retry on its own.
//!
//! The session lives on the dataset's drop root, beside the deliveries it
//! becomes: `uploads/<upload_id>/session.json` plus one `part-NNNNN` file per
//! received part. Completion concatenates the parts into the drop root's
//! staging name, checks the declared size and SHA-256, and hands the result to
//! the same `finish_delivery` a JSON push or file import uses — so the receipt,
//! validation and execution are identical, and finalising twice converges on
//! one delivery through the Idempotency-Key that #1011 introduced.
//!
//! Nothing here is reachable from another dataset: the routes resolve the
//! dataset first and read only under ITS drop root, and the session records
//! the dataset id it was opened for, which completion re-checks.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The largest part a client may send, and so the request a part costs.
pub const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;
/// The most parts one upload may declare.
pub const MAX_PARTS: u64 = 10_000;
/// How long an open session accepts parts. An expired session refuses every
/// operation but abort, and is swept the next time an upload opens.
pub const SESSION_TTL_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub upload_id: String,
    pub dataset_id: String,
    pub source: String,
    pub bytes: u64,
    pub sha256: String,
    pub part_size: u64,
    pub part_count: u64,
    /// Unix seconds.
    pub expires_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("no such upload")]
    NotFound,
    #[error("the upload expired; open a new one")]
    Expired,
    #[error("{0}")]
    Invalid(String),
    #[error("upload storage failed")]
    Io(#[from] std::io::Error),
    #[error("upload session is unreadable")]
    Json(#[from] serde_json::Error),
}

fn invalid(msg: impl Into<String>) -> UploadError {
    UploadError::Invalid(msg.into())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn safe_id(id: &str) -> bool {
    id.len() == er_ids::UUID_LENGTH
        && id
            .bytes()
            .all(|c| c == b'-' || c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn uploads_dir(root: &Path) -> PathBuf {
    root.join("uploads")
}

fn session_dir(root: &Path, upload_id: &str) -> Result<PathBuf, UploadError> {
    if !safe_id(upload_id) {
        return Err(UploadError::NotFound);
    }
    Ok(uploads_dir(root).join(upload_id))
}

fn part_path(dir: &Path, part: u64) -> PathBuf {
    dir.join(format!("part-{part:05}"))
}

/// The size part `n` (1-based) must have.
#[must_use]
pub fn expected_part_bytes(session: &Session, part: u64) -> u64 {
    if part < session.part_count {
        session.part_size
    } else {
        session.bytes - session.part_size * (session.part_count - 1)
    }
}

/// Open a session. `part_size` is the server's choice, bounded by the part cap
/// and the deployment's import cap so a part is never larger than one request
/// may be. Sweeps this drop root's expired sessions first, which is the orphan
/// cleanup: an abandoned upload holds its parts for at most one TTL plus the
/// time until the next upload opens.
///
/// # Errors
/// Refuses an empty or oversized upload or a malformed digest.
pub fn open(
    root: &Path,
    dataset_id: &str,
    source: &str,
    bytes: u64,
    sha256: &str,
    part_cap: u64,
) -> Result<Session, UploadError> {
    if bytes == 0 {
        return Err(invalid("bytes must be positive"));
    }
    if sha256.len() != 64 || !sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid("sha256 must be 64 hexadecimal characters"));
    }
    let part_size = part_cap.clamp(1, MAX_PART_BYTES);
    let part_count = bytes.div_ceil(part_size);
    if part_count > MAX_PARTS {
        return Err(invalid(format!(
            "{bytes} bytes needs {part_count} parts of {part_size}; the limit is {MAX_PARTS} \
             parts ({} bytes)",
            MAX_PARTS * part_size
        )));
    }
    sweep_expired(root);
    let session = Session {
        upload_id: er_ids::new_v7(),
        dataset_id: dataset_id.to_owned(),
        source: source.to_owned(),
        bytes,
        sha256: sha256.to_ascii_lowercase(),
        part_size,
        part_count,
        expires_at: now() + SESSION_TTL_SECS,
    };
    let dir = session_dir(root, &session.upload_id)?;
    fs::create_dir_all(&dir)?;
    let tmp = dir.join(".session.json.part");
    fs::write(&tmp, serde_json::to_vec(&session)?)?;
    fs::rename(tmp, dir.join("session.json"))?;
    Ok(session)
}

/// Read a session that belongs to `dataset_id`; another dataset's id reads as
/// absent rather than forbidden, exactly like every other cross-scope lookup.
///
/// # Errors
/// `NotFound` for an unknown or foreign id, `Expired` past its TTL.
pub fn load(root: &Path, dataset_id: &str, upload_id: &str) -> Result<Session, UploadError> {
    let dir = session_dir(root, upload_id)?;
    let session: Session = match File::open(dir.join("session.json")) {
        Ok(f) => serde_json::from_reader(f)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(UploadError::NotFound),
        Err(e) => return Err(e.into()),
    };
    if session.dataset_id != dataset_id {
        return Err(UploadError::NotFound);
    }
    if session.expires_at <= now() {
        return Err(UploadError::Expired);
    }
    Ok(session)
}

/// The part numbers already stored, ascending: what a resuming client skips.
///
/// # Errors
/// Propagates a directory read failure.
pub fn received(root: &Path, session: &Session) -> Result<Vec<u64>, UploadError> {
    let dir = session_dir(root, &session.upload_id)?;
    let mut parts: Vec<u64> = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix("part-"))
                .and_then(|n| n.parse().ok())
        })
        .collect();
    parts.sort_unstable();
    Ok(parts)
}

/// Validate a part number before any byte is read.
///
/// # Errors
/// A part outside `1..=part_count`.
pub fn check_part(session: &Session, part: u64) -> Result<(), UploadError> {
    if part == 0 || part > session.part_count {
        return Err(invalid(format!(
            "part {part} is out of range; this upload has parts 1..={}",
            session.part_count
        )));
    }
    Ok(())
}

/// Where a part's bytes are streamed before [`commit_part`] checks them.
///
/// # Errors
/// Propagates the session directory lookup.
pub fn part_staging(root: &Path, session: &Session, part: u64) -> Result<PathBuf, UploadError> {
    Ok(
        session_dir(root, &session.upload_id)?
            .join(format!(".part-{part:05}.{}", er_ids::new_v7())),
    )
}

/// Accept a streamed part: it must be exactly its expected size. A re-sent
/// part replaces the earlier copy atomically, so a retry after a lost response
/// is always safe. Returns the part's SHA-256 so a client can check its copy.
///
/// # Errors
/// A wrong size (the staged file is removed), or storage failure.
pub fn commit_part(
    root: &Path,
    session: &Session,
    part: u64,
    staged: &Path,
) -> Result<(u64, String), UploadError> {
    let expected = expected_part_bytes(session, part);
    let (bytes, digest) = digest_file(staged)?;
    if bytes != expected {
        let _ = fs::remove_file(staged);
        return Err(invalid(format!(
            "part {part} must be {expected} bytes, got {bytes}"
        )));
    }
    File::open(staged)?.sync_all()?;
    fs::rename(
        staged,
        part_path(&session_dir(root, &session.upload_id)?, part),
    )?;
    Ok((bytes, digest))
}

/// Concatenate every part into `dest` and check the whole against the
/// session's declared size and digest. On any refusal `dest` is removed and
/// the parts are kept, so the client can re-send a bad part and complete again.
///
/// # Errors
/// Missing parts (named), a size or digest mismatch, or storage failure.
pub fn assemble(root: &Path, session: &Session, dest: &Path) -> Result<(), UploadError> {
    let have = received(root, session)?;
    let missing: Vec<u64> = (1..=session.part_count)
        .filter(|p| have.binary_search(p).is_err())
        .collect();
    if !missing.is_empty() {
        let shown: Vec<String> = missing.iter().take(20).map(u64::to_string).collect();
        return Err(invalid(format!(
            "{} of {} parts are missing: {}{}",
            missing.len(),
            session.part_count,
            shown.join(", "),
            if missing.len() > 20 { ", …" } else { "" }
        )));
    }
    let dir = session_dir(root, &session.upload_id)?;
    let result = (|| -> Result<(), UploadError> {
        let mut out = OpenOptions::new().write(true).create_new(true).open(dest)?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        for part in 1..=session.part_count {
            let mut f = File::open(part_path(&dir, part))?;
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])?;
                total += n as u64;
            }
        }
        out.sync_all()?;
        let digest = hex::encode(hasher.finalize());
        if total != session.bytes {
            return Err(invalid(format!(
                "the parts total {total} bytes; the upload declared {}",
                session.bytes
            )));
        }
        if digest != session.sha256 {
            return Err(invalid(format!(
                "the assembled file's sha256 is {digest}; the upload declared {}. Re-send the \
                 part that differs from your copy (each part's sha256 is in its response)",
                session.sha256
            )));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

/// Record a completed upload's receipt and reclaim its parts. The session
/// itself stays until it expires, so a `complete` retried after a lost
/// response is answered from the receipt instead of a 404.
///
/// # Errors
/// Propagates a write or removal failure.
pub fn finish(
    root: &Path,
    session: &Session,
    delivery_id: &str,
    job_id: &str,
) -> Result<(), UploadError> {
    let dir = session_dir(root, &session.upload_id)?;
    let tmp = dir.join(".receipt.json.part");
    fs::write(&tmp, serde_json::to_vec(&(delivery_id, job_id))?)?;
    fs::rename(tmp, dir.join("receipt.json"))?;
    for part in 1..=session.part_count {
        match fs::remove_file(part_path(&dir, part)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// A completed upload's `(delivery_id, job_id)`, if it has one.
///
/// # Errors
/// Propagates a read failure other than absence.
pub fn receipt(root: &Path, session: &Session) -> Result<Option<(String, String)>, UploadError> {
    let path = session_dir(root, &session.upload_id)?.join("receipt.json");
    match File::open(path) {
        Ok(f) => Ok(Some(serde_json::from_reader(f)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Remove a session and its parts. Idempotent.
///
/// # Errors
/// Propagates a removal failure other than absence.
pub fn remove(root: &Path, upload_id: &str) -> Result<(), UploadError> {
    match fs::remove_dir_all(session_dir(root, upload_id)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Remove every expired session under this drop root.
fn sweep_expired(root: &Path) {
    let Ok(entries) = fs::read_dir(uploads_dir(root)) else {
        return;
    };
    let now = now();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path().join("session.json");
        let expired = File::open(&path)
            .ok()
            .and_then(|f| serde_json::from_reader::<_, Session>(f).ok())
            .is_some_and(|s| s.expires_at <= now);
        if expired {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn digest_file(path: &Path) -> Result<(u64, String), UploadError> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((total, hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(b: &[u8]) -> String {
        hex::encode(Sha256::digest(b))
    }

    #[test]
    fn parts_resume_assemble_and_refuse_mismatches() {
        let root = std::env::temp_dir().join(format!("er_uploads_unit_{}", er_ids::new_v7()));
        fs::create_dir_all(&root).unwrap();
        let data = b"crm_id,first_name\nC1,ada\nC2,alan\n";
        let s = open(&root, "ds1", "crm", data.len() as u64, &sha(data), 10).unwrap();
        assert_eq!(s.part_count, 4);
        assert_eq!(expected_part_bytes(&s, 4), data.len() as u64 - 30);
        // Another dataset cannot see it.
        assert!(matches!(
            load(&root, "ds2", &s.upload_id),
            Err(UploadError::NotFound)
        ));
        let put = |part: u64, bytes: &[u8]| {
            let staged = part_staging(&root, &s, part).unwrap();
            fs::write(&staged, bytes).unwrap();
            commit_part(&root, &s, part, &staged)
        };
        let chunks: Vec<&[u8]> = data.chunks(10).collect();
        put(1, chunks[0]).unwrap();
        put(3, chunks[2]).unwrap();
        assert_eq!(received(&root, &s).unwrap(), vec![1, 3]);
        // A wrong-size part is refused and leaves nothing behind.
        assert!(put(2, b"short").is_err());
        assert_eq!(received(&root, &s).unwrap(), vec![1, 3]);
        let dest = root.join("out.csv");
        let missing = assemble(&root, &s, &dest).unwrap_err().to_string();
        assert!(
            missing.contains("2 of 4 parts are missing: 2, 4"),
            "{missing}"
        );
        // A part with the right size but wrong bytes fails the digest.
        put(2, b"XXXXXXXXXX").unwrap();
        put(4, chunks[3]).unwrap();
        let bad = assemble(&root, &s, &dest).unwrap_err().to_string();
        assert!(bad.contains("sha256"), "{bad}");
        assert!(!dest.exists());
        // Re-sending the bad part fixes it.
        put(2, chunks[1]).unwrap();
        assemble(&root, &s, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        remove(&root, &s.upload_id).unwrap();
        assert!(matches!(
            load(&root, "ds1", &s.upload_id),
            Err(UploadError::NotFound)
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn opening_refuses_bad_declarations_and_sweeps_expired_sessions() {
        let root = std::env::temp_dir().join(format!("er_uploads_unit_{}", er_ids::new_v7()));
        fs::create_dir_all(&root).unwrap();
        assert!(open(&root, "d", "crm", 0, &sha(b"x"), 10).is_err());
        assert!(open(&root, "d", "crm", 5, "nothex", 10).is_err());
        assert!(open(&root, "d", "crm", MAX_PARTS * 10 + 1, &sha(b"x"), 10).is_err());
        let mut stale = open(&root, "d", "crm", 5, &sha(b"x"), 10).unwrap();
        stale.expires_at = 0;
        let dir = session_dir(&root, &stale.upload_id).unwrap();
        fs::write(
            dir.join("session.json"),
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            load(&root, "d", &stale.upload_id),
            Err(UploadError::Expired)
        ));
        open(&root, "d", "crm", 5, &sha(b"x"), 10).unwrap();
        assert!(!dir.exists(), "the expired session was swept");
        let _ = fs::remove_dir_all(&root);
    }
}
