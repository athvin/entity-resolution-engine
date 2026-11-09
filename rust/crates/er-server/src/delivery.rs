//! Immutable accepted inputs. A queued job carries a manifest, never a path or
//! a mutable "latest" pointer. Publication precedes enqueue/cursor movement;
//! neither success nor failure deletes published data needed by a retry.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use er_config::SourceSpec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The on-disk manifest and the exact reference persisted in job params.
/// Source mapping order is part of the fingerprint: ingest hashes columns in
/// declaration order, unlike the whole-config fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryPin {
    pub version: u32,
    pub dataset_id: String,
    pub source: String,
    pub delivery_id: String,
    pub bytes: u64,
    pub sha256: String,
    pub source_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("{0}")]
    Refused(&'static str),
    #[error("delivery file operation failed")]
    Io(#[from] std::io::Error),
    #[error("delivery manifest is invalid")]
    Json(#[from] serde_json::Error),
}

fn refused(message: &'static str) -> DeliveryError {
    DeliveryError::Refused(message)
}

pub(crate) fn safe_source(source: &str) -> bool {
    !source.is_empty()
        && source
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn safe_id(id: &str) -> bool {
    id.len() == er_ids::UUID_LENGTH
        && id.bytes().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
            }
        })
}

fn identity(source: &str, id: &str) -> Result<(), DeliveryError> {
    if !safe_source(source) || !safe_id(id) {
        return Err(refused("invalid delivery identity"));
    }
    Ok(())
}

fn mapping_hash(spec: &SourceSpec) -> Result<String, DeliveryError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(spec)?)))
}

fn regular_file(path: &Path) -> Result<(), DeliveryError> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(refused("delivery must be a regular file, not a symlink"));
    }
    Ok(())
}

fn directory(root: &Path, create: bool) -> Result<PathBuf, DeliveryError> {
    let dir = root.join("deliveries");
    if create {
        match fs::create_dir(&dir) {
            Ok(()) => File::open(root)?.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    if !fs::symlink_metadata(&dir)?.file_type().is_dir() {
        return Err(refused("delivery directory must not be a symlink"));
    }
    Ok(dir)
}

fn digest(path: &Path) -> Result<(u64, String), DeliveryError> {
    regular_file(path)?;
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0_u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        bytes += n as u64;
    }
    Ok((bytes, hex::encode(hash.finalize())))
}

impl DeliveryPin {
    /// Durable enqueue parameters shared by every producer.
    #[must_use]
    pub fn params(&self) -> serde_json::Value {
        serde_json::json!({
            "source": self.source, "delivery_id": self.delivery_id, "delivery": self,
        })
    }

    /// Validate scope and mapping before opening any delivery file.
    ///
    /// # Errors
    /// Refuses unknown versions, unsafe ids, a different dataset or mapping.
    pub fn validate(&self, dataset_id: &str, spec: &SourceSpec) -> Result<(), DeliveryError> {
        identity(&self.source, &self.delivery_id)?;
        if self.version != 1 || self.dataset_id != dataset_id {
            return Err(refused(
                "delivery version or dataset does not match the job",
            ));
        }
        if self.source_sha256 != mapping_hash(spec)? {
            return Err(refused(
                "source mapping changed since delivery acceptance; restore it before replay",
            ));
        }
        Ok(())
    }

    /// Read only this delivery, checking both the persisted manifest and bytes.
    ///
    /// # Errors
    /// Missing, replaced, corrupt or out-of-scope inputs are terminal refusals;
    /// none falls back to a source's latest file.
    pub fn read(
        &self,
        root: &Path,
        dataset_id: &str,
        spec: &SourceSpec,
    ) -> Result<String, DeliveryError> {
        self.validate(dataset_id, spec)?;
        let dir = directory(root, false)?;
        let manifest = dir.join(format!("{}.json", self.delivery_id));
        regular_file(&manifest)?;
        let stored: Self = serde_json::from_reader(File::open(manifest)?)?;
        if stored != *self {
            return Err(refused(
                "queued delivery does not match its persisted manifest",
            ));
        }
        let path = dir.join(format!("{}.csv", self.delivery_id));
        regular_file(&path)?;
        let mut csv = String::new();
        File::open(path)?.read_to_string(&mut csv)?;
        if csv.len() as u64 != self.bytes
            || hex::encode(Sha256::digest(csv.as_bytes())) != self.sha256
        {
            return Err(refused(
                "delivery bytes no longer match their accepted digest",
            ));
        }
        Ok(csv)
    }
}

/// Publish a closed staging file without replacing any existing delivery.
/// The caller must durably enqueue `pin.params()` before advancing a cursor.
/// Staging names are derived here, never accepted as client-supplied paths.
///
/// # Errors
/// Refuses an unsafe identity, conflicting publication or filesystem failure.
/// Files survive failure so an operator can replay a manifest after an enqueue
/// failure; a retry of publication with the same identity is also safe.
pub fn publish(
    root: &Path,
    dataset_id: &str,
    source: &str,
    delivery_id: &str,
    spec: &SourceSpec,
) -> Result<DeliveryPin, DeliveryError> {
    identity(source, delivery_id)?;
    if dataset_id.is_empty() {
        return Err(refused("delivery requires a dataset"));
    }
    let dir = directory(root, true)?;
    let part = root.join(format!(".{source}.{delivery_id}.csv.part"));
    let manifest = dir.join(format!("{delivery_id}.json"));
    // A completed publish can be retried after its staging name was removed.
    if !part.try_exists()? && manifest.try_exists()? {
        regular_file(&manifest)?;
        let pin: DeliveryPin = serde_json::from_reader(File::open(&manifest)?)?;
        if pin.source != source || pin.delivery_id != delivery_id {
            return Err(refused("published delivery identity conflicts with retry"));
        }
        pin.read(root, dataset_id, spec)?;
        return Ok(pin);
    }
    regular_file(&part)?;
    File::open(&part)?.sync_all()?;
    let (bytes, sha256) = digest(&part)?;
    let pin = DeliveryPin {
        version: 1,
        dataset_id: dataset_id.to_owned(),
        source: source.to_owned(),
        delivery_id: delivery_id.to_owned(),
        bytes,
        sha256,
        source_sha256: mapping_hash(spec)?,
    };
    let live = dir.join(format!("{delivery_id}.csv"));
    // hard_link is an atomic, no-clobber publication on the shared filesystem.
    // No mutable source.csv alias is ever linked to this inode.
    match fs::hard_link(&part, &live) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if digest(&live)? != (pin.bytes, pin.sha256.clone()) {
                return Err(refused("delivery identity already has different bytes"));
            }
        }
        Err(e) => return Err(e.into()),
    }
    File::open(&dir)?.sync_all()?;
    let manifest_part = dir.join(format!(".{}.json.part", er_ids::new_v7()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest_part)?;
    file.write_all(&serde_json::to_vec(&pin)?)?;
    file.sync_all()?;
    drop(file);
    let publication = fs::hard_link(&manifest_part, &manifest);
    let _ = fs::remove_file(&manifest_part);
    match publication {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            regular_file(&manifest)?;
            let existing: DeliveryPin = serde_json::from_reader(File::open(&manifest)?)?;
            if existing != pin {
                return Err(refused(
                    "delivery identity already has a different manifest",
                ));
            }
        }
        Err(e) => return Err(e.into()),
    }
    File::open(&dir)?.sync_all()?;
    fs::remove_file(part)?;
    File::open(root)?.sync_all()?;
    Ok(pin)
}
