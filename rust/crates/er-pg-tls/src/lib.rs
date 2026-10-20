//! `er-pg-tls` — the TLS connector for every Postgres client in the workspace.
//!
//! Aurora DSNs carry `sslmode=require`, which `tokio_postgres::NoTls` refuses
//! outright. This connector gives `require` its libpq semantics: the stream is
//! encrypted but the server's certificate is NOT verified (RDS certs chain to
//! Amazon's RDS CA, not the system roots, and the infrastructure decision is to
//! run `require` without a CA bundle — infrastructure.md §12). Plain `prefer`
//! DSNs (the local docker test container) still degrade to cleartext when the
//! server declines TLS, so tests and dev lakes are unaffected.
//!
//! `verify-full` against the RDS trust bundle is a prod-gate hardening item,
//! not a dev one.

use std::sync::Arc;

use rustls::ClientConfig;
use rustls::DigitallySignedStruct;
use rustls::SignatureScheme;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

/// Accept any server certificate (libpq `sslmode=require` parity): the
/// handshake signatures still have to verify, only the chain-of-trust and
/// name checks are skipped.
#[derive(Debug)]
struct AcceptAnyServerCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// The connector every `tokio_postgres::connect` in the workspace passes where
/// `NoTls` used to go.
#[must_use]
pub fn make_tls() -> tokio_postgres_rustls::MakeRustlsConnect {
    let provider = Arc::new(ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default TLS versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert(provider)))
        .with_no_client_auth();
    tokio_postgres_rustls::MakeRustlsConnect::new(config)
}
