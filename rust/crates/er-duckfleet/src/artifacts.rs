//! Immutable, dataset-scoped file references. Credentials and caller-supplied
//! URLs never enter this contract. Workers may read sealed objects and write
//! staging objects; only the control plane can publish a sealed artifact.

use serde::{Deserialize, Serialize};

use crate::{Binding, identifier};

/// Matches the accepted delivery size limit. Larger exports must be partitioned
/// into independently sealed files, never returned through the SQL response.
pub const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

/// Closed vocabulary also determines the extension interpreted by the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Csv,
    Json,
    Yaml,
    Parquet,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Parquet => "parquet",
        }
    }
}

/// A reference is published only after all bytes have been verified. A digest
/// in a key identifies content, not permission: every use validates its scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    version: u32,
    org_id: String,
    dataset_id: String,
    identity: String,
    format: Format,
    bytes: u64,
    sha256: String,
}

/// Fixed messages deliberately exclude object keys, credentials and S3 errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid or out-of-scope DuckFleet artifact")]
pub struct InvalidArtifact;

impl Artifact {
    /// Construct after verifying the complete object. `identity` is a delivery,
    /// configuration version or result identity, never a customer file path.
    pub fn new(
        binding: &Binding<'_>,
        identity: &str,
        format: Format,
        bytes: u64,
        sha256: &str,
    ) -> Result<Self, InvalidArtifact> {
        let artifact = Self {
            version: 1,
            org_id: binding.org_id().to_owned(),
            dataset_id: binding.dataset_id().to_owned(),
            identity: identity.to_owned(),
            format,
            bytes,
            sha256: sha256.to_owned(),
        };
        artifact.validate(binding)?;
        Ok(artifact)
    }

    /// Validate deserialized metadata before any object-store or SQL operation.
    pub fn validate(&self, binding: &Binding<'_>) -> Result<(), InvalidArtifact> {
        if self.version != 1
            || self.org_id != binding.org_id()
            || self.dataset_id != binding.dataset_id()
            || !identifier(&self.identity)
            || self.bytes > MAX_ARTIFACT_BYTES
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InvalidArtifact);
        }
        Ok(())
    }

    /// Content-addressed key under the immutable storage policy. Generation is
    /// intentionally absent: rotating credentials must not invalidate a retry.
    pub fn key(&self, binding: &Binding<'_>) -> Result<String, InvalidArtifact> {
        self.validate(binding)?;
        Ok(format!(
            "duckfleet/{}/artifacts/immutable/{}/{}.{}",
            binding.namespace(),
            self.identity,
            self.sha256,
            self.format.extension()
        ))
    }

    /// Unsigned worker path; credentials come from the worker's own identity.
    pub fn uri(&self, binding: &Binding<'_>) -> Result<String, InvalidArtifact> {
        Ok(format!("s3://{}/{}", binding.bucket(), self.key(binding)?))
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn format(&self) -> Format {
        self.format
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

/// An attempt-specific output location. It is never an accepted artifact and
/// cannot become one until the publisher has downloaded and checked its bytes.
pub struct Staging {
    org_id: String,
    dataset_id: String,
    attempt: uuid::Uuid,
    format: Format,
}

impl Staging {
    pub fn new(binding: &Binding<'_>, attempt: uuid::Uuid, format: Format) -> Self {
        Self {
            org_id: binding.org_id().to_owned(),
            dataset_id: binding.dataset_id().to_owned(),
            attempt,
            format,
        }
    }

    pub fn key(&self, binding: &Binding<'_>) -> Result<String, InvalidArtifact> {
        if self.org_id != binding.org_id() || self.dataset_id != binding.dataset_id() {
            return Err(InvalidArtifact);
        }
        Ok(format!(
            "duckfleet/{}/artifacts/staging/{}.{}",
            binding.namespace(),
            self.attempt,
            self.format.extension()
        ))
    }

    pub fn uri(&self, binding: &Binding<'_>) -> Result<String, InvalidArtifact> {
        Ok(format!("s3://{}/{}", binding.bucket(), self.key(binding)?))
    }

    pub fn format(&self) -> Format {
        self.format
    }
}
