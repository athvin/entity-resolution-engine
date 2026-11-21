//! Operator-owned dataset bindings for DuckFleet. HTTP callers never supply an
//! endpoint, catalog, storage prefix, worker identity or credential reference.
//!
//! Load a fresh registry and projected credential snapshot before admitting an
//! operation. Revocation prevents new admissions; the operator also revokes the
//! upstream token and drains existing sessions before completing revocation.

pub mod artifacts;
pub mod execution;

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Deliberately fixed errors: parsing a secret must never echo its input.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid DuckFleet binding registry")]
    Registry,
    #[error("DuckFleet dataset binding unavailable")]
    Unavailable,
    #[error("invalid DuckFleet credential projection")]
    Credential,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    bucket: String,
    bindings: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    org_id: String,
    dataset_id: String,
    generation: u64,
    state: State,
    valid_until: u64,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum State {
    Provisioning,
    Active,
    Draining,
    Revoked,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    org_id: String,
    dataset_id: String,
    generation: u64,
    token: String,
}

/// An immutable admission snapshot. No Debug or Serialize implementation: the
/// snapshot owns bearer credentials. Reload before each operation, not once at
/// process startup, so a projected registry update revokes future admissions.
pub struct Registry {
    bucket: String,
    entries: Vec<(Entry, Option<Credential>)>,
}

/// A resolved capability for exactly one dataset. All resource names are
/// derived from immutable IDs; none are accepted from job or HTTP payloads.
/// This type deliberately cannot be serialized or formatted with Debug.
pub struct Binding<'a> {
    entry: &'a Entry,
    credential: &'a Credential,
    bucket: &'a str,
    namespace: String,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn bucket_name(value: &str) -> bool {
    (3..=63).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
}

/// Stable namespace shared with the Terraform module. A NUL separator makes
/// different org/dataset pairs unambiguous. IDs may not contain a NUL.
pub fn namespace(org_id: &str, dataset_id: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(org_id.as_bytes());
    hash.update([0]);
    hash.update(dataset_id.as_bytes());
    format!("duckfleet-{}", &hex::encode(hash.finalize())[..40])
}

impl Registry {
    /// Load operator-owned files. `credentials` is the projected-secret mount,
    /// with `<namespace>/credential.json` for each active deployment. Neither
    /// path may originate in a request or dataset configuration.
    ///
    /// An invalid active entry refuses the entire snapshot. Inactive entries
    /// need no credential, allowing provisioning and recoverable revocation;
    /// a draining entry's credential is loaded when present, for recovery only.
    pub fn load(registry: &Path, credentials: &Path) -> Result<Self, Error> {
        let json = std::fs::read(registry).map_err(|_| Error::Registry)?;
        Self::parse(&json, |ns| {
            std::fs::read(credentials.join(ns).join("credential.json"))
                .map_err(|_| Error::Credential)
        })
    }

    fn parse(
        json: &[u8],
        mut read_credential: impl FnMut(&str) -> Result<Vec<u8>, Error>,
    ) -> Result<Self, Error> {
        let document: Document = serde_json::from_slice(json).map_err(|_| Error::Registry)?;
        if !bucket_name(&document.bucket) {
            return Err(Error::Registry);
        }
        let mut datasets = BTreeSet::new();
        let mut namespaces = BTreeSet::new();
        let mut tokens = BTreeSet::new();
        let mut entries = Vec::with_capacity(document.bindings.len());
        for entry in document.bindings {
            let ns = namespace(&entry.org_id, &entry.dataset_id);
            if !identifier(&entry.org_id)
                || !identifier(&entry.dataset_id)
                || entry.generation == 0
                || entry.valid_until == 0
                || !datasets.insert(entry.dataset_id.clone())
                || !namespaces.insert(ns.clone())
            {
                return Err(Error::Registry);
            }
            // A draining binding admits nothing new, but its credential is
            // still how outstanding attempts are reconciled, so it is loaded
            // when projected. Absence is fine; a malformed one still refuses.
            let bytes = match entry.state {
                State::Active => Some(read_credential(&ns)?),
                State::Draining => read_credential(&ns).ok(),
                State::Provisioning | State::Revoked => None,
            };
            let credential = if let Some(bytes) = bytes {
                let credential: Credential =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Credential)?;
                if credential.org_id != entry.org_id
                    || credential.dataset_id != entry.dataset_id
                    || credential.generation != entry.generation
                    || credential.token.len() < 32
                    || credential.token.len() > 4096
                    || !credential.token.bytes().all(|b| b.is_ascii_graphic())
                    || !tokens.insert(Sha256::digest(credential.token.as_bytes()).to_vec())
                {
                    return Err(Error::Credential);
                }
                Some(credential)
            } else {
                None
            };
            entries.push((entry, credential));
        }
        Ok(Self {
            bucket: document.bucket,
            entries,
        })
    }

    /// Admit a new operation at the current operator-owned generation. Later
    /// calls retain this capability; never resolve a replacement after loss.
    pub fn resolve_current(
        &self,
        org_id: &str,
        dataset_id: &str,
        now_unix_seconds: u64,
    ) -> Result<Binding<'_>, Error> {
        let generation = self
            .entries
            .iter()
            .find(|(entry, _)| entry.org_id == org_id && entry.dataset_id == dataset_id)
            .map(|(entry, _)| entry.generation)
            .ok_or(Error::Unavailable)?;
        self.resolve(org_id, dataset_id, generation, now_unix_seconds)
    }

    /// Resolve only after the control plane authorizes and loads the dataset.
    /// `generation` is the server-recorded attempt/session generation, never a
    /// customer-selected identity. Missing, stale and inactive bindings all
    /// refuse; there is no fallback to local compute or a default deployment.
    pub fn resolve(
        &self,
        org_id: &str,
        dataset_id: &str,
        generation: u64,
        now_unix_seconds: u64,
    ) -> Result<Binding<'_>, Error> {
        let (entry, credential) = self
            .entries
            .iter()
            .find(|(entry, _)| entry.org_id == org_id && entry.dataset_id == dataset_id)
            .ok_or(Error::Unavailable)?;
        if entry.state != State::Active
            || entry.generation != generation
            || now_unix_seconds >= entry.valid_until
        {
            return Err(Error::Unavailable);
        }
        Ok(Binding {
            entry,
            credential: credential.as_ref().ok_or(Error::Unavailable)?,
            bucket: &self.bucket,
            namespace: namespace(org_id, dataset_id),
        })
    }
}

impl Registry {
    /// Resolve the exact generation an earlier attempt was admitted under, for
    /// reading recovery evidence only. A draining binding still resolves —
    /// draining is when outstanding attempts must be reconciled — but a revoked
    /// one or a replacement generation never does, so evidence is never read
    /// through a different principal (whose owner-scoped lookups would report
    /// a request as absent). Opens nothing; admits no new work.
    pub fn resolve_for_recovery(
        &self,
        org_id: &str,
        dataset_id: &str,
        generation: u64,
    ) -> Result<Binding<'_>, Error> {
        let (entry, credential) = self
            .entries
            .iter()
            .find(|(entry, _)| entry.org_id == org_id && entry.dataset_id == dataset_id)
            .ok_or(Error::Unavailable)?;
        if !matches!(entry.state, State::Active | State::Draining) || entry.generation != generation
        {
            return Err(Error::Unavailable);
        }
        Ok(Binding {
            entry,
            credential: credential.as_ref().ok_or(Error::Unavailable)?,
            bucket: &self.bucket,
            namespace: namespace(org_id, dataset_id),
        })
    }
}

impl Binding<'_> {
    /// Operator-owned bucket. Object keys must still be derived from this binding.
    pub fn bucket(&self) -> &str {
        self.bucket
    }
    /// Dataset-scoped internal gateway. Never expose it as a customer API.
    pub fn endpoint(&self) -> String {
        format!("http://server.{}.svc.cluster.local:8080", self.namespace)
    }

    /// Namespace owning compute, catalog, sessions and query history.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Immutable dataset ID, suitable for authorization comparisons.
    pub fn dataset_id(&self) -> &str {
        &self.entry.dataset_id
    }

    /// Immutable organization ID, suitable for authorization comparisons.
    pub fn org_id(&self) -> &str {
        &self.entry.org_id
    }

    /// Generation fences sessions and queued attempts across credential rotation.
    pub fn generation(&self) -> u64 {
        self.entry.generation
    }

    /// The only object-storage root granted to the deployment's worker identity.
    /// Lake data and transfer artifacts occupy separate children of this root.
    pub fn storage_root(&self) -> String {
        format!("s3://{}/duckfleet/{}/", self.bucket, self.namespace)
    }

    /// Token for the Authorization header only. Never use in SQL, telemetry,
    /// org configuration, a job payload or a persisted error message.
    pub fn bearer_token(&self) -> &str {
        &self.credential.token
    }
}

#[cfg(test)]
mod tests;
