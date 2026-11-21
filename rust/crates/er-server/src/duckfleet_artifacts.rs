//! Bounded artifact transport for the operator-only DuckFleet backend. The
//! accepted delivery receipt remains authoritative. SQL carries unsigned object
//! paths; data, credentials and signed URLs never travel in query responses.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use aws_sdk_s3::{Client, primitives::ByteStream};
use base64::Engine;
use er_duckfleet::{
    Binding,
    artifacts::{Artifact, Format, InvalidArtifact, MAX_ARTIFACT_BYTES, Staging},
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::delivery::DeliveryPin;

const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);

/// Fixed errors keep SDK responses (which can contain keys and signed request
/// details) out of telemetry. The caller records the operation and outcome.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("DuckFleet artifact metadata refused")]
    Metadata(#[from] InvalidArtifact),
    #[error("DuckFleet accepted delivery refused")]
    Delivery(#[from] crate::delivery::DeliveryError),
    #[error("DuckFleet artifact local I/O failed")]
    Io(#[from] std::io::Error),
    #[error("DuckFleet artifact storage operation failed")]
    Storage,
    #[error("DuckFleet artifact content does not match its accepted identity")]
    Integrity,
    #[error("DuckFleet artifact exceeds the file size limit")]
    TooLarge,
    #[error("DuckFleet artifact preparation interrupted")]
    Interrupted,
}

/// Caller supplies a client built with the control plane's Pod Identity and
/// ring-backed transport. A worker's client cannot publish immutable objects.
pub struct Store {
    client: Client,
}

struct Snapshot {
    file: tempfile::NamedTempFile,
    bytes: u64,
    sha256: String,
}

fn snapshot(mut input: impl Read) -> Result<Snapshot, Error> {
    let mut file = tempfile::NamedTempFile::new()?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 65536];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > MAX_ARTIFACT_BYTES {
            return Err(Error::TooLarge);
        }
        hash.update(&buffer[..n]);
        file.write_all(&buffer[..n])?;
    }
    file.flush()?;
    Ok(Snapshot {
        file,
        bytes,
        sha256: hex::encode(hash.finalize()),
    })
}

impl Store {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Bridge a durable delivery without changing its receipt or consuming the
    /// accepted local copy. A failed or uncertain upload leaves replay intact.
    #[tracing::instrument(skip_all, fields(operation = "duckfleet.artifact.delivery"))]
    pub async fn publish_delivery(
        &self,
        binding: &Binding<'_>,
        pin: &DeliveryPin,
        root: &Path,
        spec: &er_config::SourceSpec,
    ) -> Result<Artifact, Error> {
        pin.validate(binding.dataset_id(), spec)?;
        let artifact = Artifact::new(
            binding,
            &pin.delivery_id,
            Format::Csv,
            pin.bytes,
            &pin.sha256,
        )?;
        let pin = pin.clone();
        let root = root.to_owned();
        let spec = spec.clone();
        let dataset = binding.dataset_id().to_owned();
        let prepared = tokio::task::spawn_blocking(move || {
            snapshot(pin.open_verified(&root, &dataset, &spec)?)
        })
        .await
        .map_err(|_| Error::Interrupted)??;
        self.publish(binding, artifact, prepared).await
    }

    /// Transfer a pinned configuration, model, dictionary or TF file. The
    /// expected fingerprint must come from accepted job metadata, never from a
    /// mutable path discovered during replay. The private snapshot prevents a
    /// concurrent local modification from changing the bytes sent by the SDK.
    #[tracing::instrument(skip_all, fields(operation = "duckfleet.artifact.input"))]
    pub async fn publish_file(
        &self,
        binding: &Binding<'_>,
        artifact: Artifact,
        file: std::fs::File,
    ) -> Result<Artifact, Error> {
        artifact.validate(binding)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != artifact.bytes() {
            return Err(Error::Integrity);
        }
        let prepared = tokio::task::spawn_blocking(move || snapshot(file))
            .await
            .map_err(|_| Error::Interrupted)??;
        self.publish(binding, artifact, prepared).await
    }

    async fn publish(
        &self,
        binding: &Binding<'_>,
        artifact: Artifact,
        prepared: Snapshot,
    ) -> Result<Artifact, Error> {
        if prepared.bytes != artifact.bytes() || prepared.sha256 != artifact.sha256() {
            return Err(Error::Integrity);
        }
        let key = artifact.key(binding)?;
        let body = ByteStream::from_path(prepared.file.path())
            .await
            .map_err(|_| Error::Storage)?;
        let digest = hex::decode(artifact.sha256()).map_err(|_| Error::Integrity)?;
        let checksum = base64::engine::general_purpose::STANDARD.encode(digest);
        let request = self
            .client
            .put_object()
            .bucket(binding.bucket())
            .key(&key)
            .if_none_match("*")
            .checksum_sha256(checksum)
            .body(body);
        let result = tokio::time::timeout(TRANSFER_TIMEOUT, request.send())
            .await
            .map_err(|_| Error::Storage)?;
        if let Err(error) = result {
            // Only an explicit precondition failure is a possible idempotent
            // repeat. Timeouts/409/403 never become success or an overwrite.
            let conflict = error
                .raw_response()
                .is_some_and(|r| r.status().as_u16() == 412);
            if !conflict {
                return Err(Error::Storage);
            }
            self.verify(binding, &artifact).await?;
        }
        Ok(artifact)
    }

    /// Download a completed worker output to a private bounded spool, hash it,
    /// then publish it under the immutable policy. Staging is never itself a
    /// result reference. The caller persists the returned manifest before
    /// reporting success; interruption leaves only unreferenced reusable bytes.
    #[tracing::instrument(skip_all, fields(operation = "duckfleet.artifact.seal"))]
    pub async fn seal(
        &self,
        binding: &Binding<'_>,
        staging: &Staging,
        identity: &str,
    ) -> Result<Artifact, Error> {
        let prepared = self.download(binding, &staging.key(binding)?).await?;
        let artifact = Artifact::new(
            binding,
            identity,
            staging.format(),
            prepared.bytes,
            &prepared.sha256,
        )?;
        self.publish(binding, artifact, prepared).await
    }

    /// Verify bytes on retry rather than trusting user-controlled S3 metadata.
    #[tracing::instrument(skip_all, fields(operation = "duckfleet.artifact.verify"))]
    pub async fn verify(&self, binding: &Binding<'_>, artifact: &Artifact) -> Result<(), Error> {
        self.fetch(binding, artifact).await?;
        Ok(())
    }

    /// Stream a sealed artifact to a private local file and verify before handing
    /// it to a consumer. No partially downloaded or corrupt file escapes.
    #[tracing::instrument(skip_all, fields(operation = "duckfleet.artifact.fetch"))]
    pub async fn fetch(
        &self,
        binding: &Binding<'_>,
        artifact: &Artifact,
    ) -> Result<tempfile::NamedTempFile, Error> {
        let prepared = self.download(binding, &artifact.key(binding)?).await?;
        if prepared.bytes != artifact.bytes() || prepared.sha256 != artifact.sha256() {
            return Err(Error::Integrity);
        }
        Ok(prepared.file)
    }

    async fn download(&self, binding: &Binding<'_>, key: &str) -> Result<Snapshot, Error> {
        tokio::time::timeout(TRANSFER_TIMEOUT, self.download_inner(binding, key))
            .await
            .map_err(|_| Error::Storage)?
    }

    async fn download_inner(&self, binding: &Binding<'_>, key: &str) -> Result<Snapshot, Error> {
        let response = self
            .client
            .get_object()
            .bucket(binding.bucket())
            .key(key)
            .send()
            .await
            .map_err(|_| Error::Storage)?;
        if response
            .content_length()
            .is_some_and(|n| n < 0 || n as u64 > MAX_ARTIFACT_BYTES)
        {
            return Err(Error::TooLarge);
        }
        let file = tempfile::NamedTempFile::new()?;
        let mut output = tokio::fs::File::from_std(file.reopen()?);
        let mut body = response.body;
        let mut bytes = 0_u64;
        let mut hash = Sha256::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| Error::Storage)?;
            bytes += chunk.len() as u64;
            if bytes > MAX_ARTIFACT_BYTES {
                return Err(Error::TooLarge);
            }
            hash.update(&chunk);
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        Ok(Snapshot {
            file,
            bytes,
            sha256: hex::encode(hash.finalize()),
        })
    }

    /// Only an attempt-specific staging object can be deleted through this API.
    /// Accepted inputs and sealed outputs have no deletion API or TTL here.
    #[tracing::instrument(skip_all, fields(operation = "duckfleet.artifact.cleanup"))]
    pub async fn remove_staging(
        &self,
        binding: &Binding<'_>,
        staging: &Staging,
    ) -> Result<(), Error> {
        let request = self
            .client
            .delete_object()
            .bucket(binding.bucket())
            .key(staging.key(binding)?);
        tokio::time::timeout(TRANSFER_TIMEOUT, request.send())
            .await
            .map_err(|_| Error::Storage)?
            .map_err(|_| Error::Storage)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_s3::config::{Credentials, Region, retry::RetryConfig};
    use er_duckfleet::Registry;
    use serde_json::json;
    use std::net::TcpListener;

    fn registry(root: &Path) -> Registry {
        let ns = er_duckfleet::namespace("org-a", "dataset-a");
        std::fs::create_dir(root.join(&ns)).unwrap();
        std::fs::write(
            root.join(ns).join("credential.json"),
            serde_json::to_vec(&json!({
                "org_id":"org-a", "dataset_id":"dataset-a", "generation":1,
                "token":"synthetic-token-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("registry.json"),
            serde_json::to_vec(&json!({
                "bucket":"fixture-bucket", "bindings":[{"org_id":"org-a", "dataset_id":"dataset-a",
                    "generation":1,"state":"active","valid_until":200}]
            }))
            .unwrap(),
        )
        .unwrap();
        Registry::load(&root.join("registry.json"), root).unwrap()
    }

    // One response then close: a deliberately truncated body must never become
    // a usable file even if S3 successfully returned its response headers.
    fn fixture(response: Vec<u8>) -> (Store, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let task = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request);
            socket.write_all(&response).unwrap();
        });
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
                aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
            ))
            .build_https();
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .endpoint_url(format!("http://{address}"))
            .force_path_style(true)
            .retry_config(RetryConfig::disabled())
            .http_client(http)
            .credentials_provider(Credentials::new("fixture", "fixture", None, None, "test"))
            .build();
        (Store::new(Client::from_conf(config)), task)
    }

    #[tokio::test]
    async fn incomplete_corrupt_and_oversized_downloads_never_escape() {
        let root = tempfile::tempdir().unwrap();
        let registry = registry(root.path());
        let binding = registry.resolve("org-a", "dataset-a", 1, 100).unwrap();
        let artifact = Artifact::new(
            &binding,
            "accepted",
            Format::Csv,
            8,
            &hex::encode(Sha256::digest(b"accepted")),
        )
        .unwrap();
        let expired =
            "<Error><Code>ExpiredToken</Code><Message>fixture session expired</Message></Error>";
        for (response, expected) in [
            (format!("HTTP/1.1 403 Forbidden\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{expired}", expired.len()).into_bytes(), "storage"),
            (
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\npart".to_vec(),
                "storage",
            ),
            (
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nreplaced"
                    .to_vec(),
                "integrity",
            ),
            (
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    MAX_ARTIFACT_BYTES + 1
                )
                .into_bytes(),
                "limit",
            ),
        ] {
            let (store, server) = fixture(response);
            let error = store.fetch(&binding, &artifact).await.unwrap_err();
            assert!(matches!(
                (expected, error),
                ("storage", Error::Storage)
                    | ("integrity", Error::Integrity)
                    | ("limit", Error::TooLarge)
            ));
            server.join().unwrap();
        }
    }
}
