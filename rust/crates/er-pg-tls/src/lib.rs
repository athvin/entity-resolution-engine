//! `er-pg-tls` — the TLS connector for every Postgres client in the workspace.
//!
//! Aurora DSNs carry `sslmode=require`, which `tokio_postgres::NoTls` refuses
//! outright. The [`make_tls`] connector gives `require` its libpq semantics:
//! the stream is encrypted but the server's certificate is NOT verified (RDS
//! certs chain to Amazon's RDS CA, not the system roots). Plain `prefer` DSNs
//! (the local docker test container) still degrade to cleartext when the
//! server declines TLS, so tests and dev lakes are unaffected.
//!
//! `verify-full` is the prod posture (#553, §3.1): chain AND hostname verified
//! against the RDS CA bundle the images carry. tokio-postgres parses only
//! `disable`/`prefer`/`require` — and errors on any query key it does not
//! know, `sslrootcert` included — so [`connect`] lifts both parameters out of
//! the DSN before tokio-postgres sees it, rewrites the wire requirement to
//! `require`, and swaps in the verifying connector. A DSN that asks for
//! `verify-full` without `sslrootcert` is a misconfiguration and errors
//! loudly; `verify-ca` (chain without a name check) is deliberately not
//! offered, because it is the middle ground nobody should pick.

use std::path::PathBuf;
use std::sync::Arc;

use rustls::ClientConfig;
use rustls::DigitallySignedStruct;
use rustls::SignatureScheme;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

/// Everything connect needs that tokio-postgres cannot parse itself.
#[derive(Debug)]
pub struct Plan {
    /// The DSN with `sslmode`/`sslrootcert` rewritten to what tokio-postgres
    /// accepts: `verify-full` becomes `require` (the wire stays TLS-mandatory)
    /// and `sslrootcert` is removed (it names a local file; the server never
    /// sees it, and tokio-postgres errors on unknown query keys).
    pub dsn: String,
    /// What the original DSN asked for.
    pub mode: SslMode,
    /// The CA bundle path for `verify-full`, verbatim from `sslrootcert`.
    pub sslrootcert: Option<PathBuf>,
}

/// The sslmode a DSN carries, including the one tokio-postgres refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    /// `disable` — tokio-postgres never invokes the connector.
    Disable,
    /// `prefer` (or no sslmode at all) — TLS attempted, cleartext fallback.
    Prefer,
    /// `require` — encrypted, not verified. Today's Aurora posture.
    Require,
    /// `verify-full` — encrypted, chain- and hostname-verified against the
    /// bundle `sslrootcert` names.
    VerifyFull,
}

/// Why a DSN could not be planned or opened.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Repeated TLS options make the requested security posture ambiguous.
    #[error("duplicate Postgres TLS option")]
    DuplicateTlsOption,
    /// A sslmode this crate deliberately does not serve.
    #[error(
        "sslmode={0} is not supported: tokio-postgres serves disable/prefer/require, this crate \
         adds verify-full, and verify-ca (chain without a name check) is deliberately not offered"
    )]
    UnsupportedSslMode(String),
    /// `verify-full` with no bundle to verify against.
    #[error("sslmode=verify-full needs sslrootcert=<path> naming the CA bundle to verify against")]
    VerifyFullNeedsRootcert,
    /// The bundle file could not be read.
    #[error("reading the CA bundle {path:?}")]
    RootcertRead {
        /// The path `sslrootcert` named.
        path: PathBuf,
        /// The I/O failure.
        source: std::io::Error,
    },
    /// The bundle read cleanly but carried no certificates.
    #[error("the CA bundle {path:?} contains no parseable PEM certificates")]
    RootcertEmpty {
        /// The path `sslrootcert` named.
        path: PathBuf,
    },
    /// The bundle's PEM framing was malformed.
    #[error("parsing PEM in the CA bundle {path:?}")]
    RootcertParse {
        /// The path `sslrootcert` named.
        path: PathBuf,
        /// The PEM decoding failure.
        source: rustls::pki_types::pem::Error,
    },
    /// The connect itself failed.
    #[error("connecting to Postgres")]
    Postgres(#[from] tokio_postgres::Error),
}

/// Split a DSN into what tokio-postgres can parse and what it cannot.
///
/// Only the URI form is inspected (every DSN in this repo is one); anything
/// that does not parse as a URI passes through unchanged, which is today's
/// behaviour for the libpq keyword form.
///
/// # Errors
/// Unsupported or duplicate TLS options and a missing verification bundle
/// fail before any connection is attempted.
pub fn plan(dsn: &str) -> Result<Plan, Error> {
    let Ok(mut url) = url::Url::parse(dsn) else {
        // The libpq keyword form ("host=… sslmode=…"): not URI-shaped. Pass
        // through rather than guess at a second grammar.
        return Ok(Plan {
            dsn: dsn.to_string(),
            mode: SslMode::Prefer,
            sslrootcert: None,
        });
    };
    let mut mode = SslMode::Prefer;
    // Postgres URI queries use percent encoding, not HTML form encoding:
    // a literal '+' must survive the form decoder used by url::Url.
    let query = url.query().map(|query| query.replace('+', "%2B"));
    url.set_query(query.as_deref());
    let mut has_sslmode = false;
    let mut sslrootcert = None;
    let mut kept: Vec<(String, String)> = Vec::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "sslmode" => {
                if has_sslmode {
                    return Err(Error::DuplicateTlsOption);
                }
                has_sslmode = true;
                mode = match value.as_ref() {
                    "disable" => SslMode::Disable,
                    "prefer" => SslMode::Prefer,
                    "require" => SslMode::Require,
                    "verify-full" => SslMode::VerifyFull,
                    other => return Err(Error::UnsupportedSslMode(other.to_string())),
                };
            }
            "sslrootcert" => {
                if sslrootcert.is_some() {
                    return Err(Error::DuplicateTlsOption);
                }
                sslrootcert = Some(PathBuf::from(value.as_ref()));
            }
            _ => kept.push((key.into_owned(), value.into_owned())),
        }
    }
    if mode == SslMode::VerifyFull
        && sslrootcert
            .as_ref()
            .is_none_or(|path| path.as_os_str().is_empty())
    {
        return Err(Error::VerifyFullNeedsRootcert);
    }
    // Rebuild the query: verify-full travels as require on the wire (the
    // connector below is what adds verification), and sslrootcert never
    // reaches tokio-postgres at all.
    url.set_query(None);
    let wire_mode = match mode {
        SslMode::VerifyFull | SslMode::Require => "require",
        SslMode::Disable => "disable",
        SslMode::Prefer => "prefer",
    };
    // Only restate what the DSN said: a DSN with no sslmode gets none, and an
    // empty serializer would leave a stray `?` behind.
    let state_sslmode = has_sslmode;
    if !kept.is_empty() || state_sslmode {
        let mut query = url.query_pairs_mut();
        for (key, value) in &kept {
            query.append_pair(key, value);
        }
        if state_sslmode {
            query.append_pair("sslmode", wire_mode);
        }
    }
    let query = url.query().map(|query| query.replace('+', "%20"));
    url.set_query(query.as_deref());
    Ok(Plan {
        dsn: url.into(),
        mode,
        sslrootcert,
    })
}

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
/// `NoTls` used to go, for DSNs at `require` or below. `verify-full` DSNs go
/// through [`make_tls_verified`] instead.
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

/// The `verify-full` connector: chain and hostname verified by rustls'
/// WebPKI verifier against exactly the bundle `rootcert` names — not the
/// system roots, which the RDS CA is not in. The provider is the same `ring`
/// one [`make_tls`] uses; a mismatch is a runtime panic, per the workspace's
/// TLS pins.
///
/// # Errors
/// `RootcertRead`/`RootcertParse`/`RootcertEmpty` if the bundle cannot be
/// read or yields no certificates.
pub fn make_tls_verified(
    rootcert: &std::path::Path,
) -> Result<tokio_postgres_rustls::MakeRustlsConnect, Error> {
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(
        verified_config(rootcert)?,
    ))
}

fn verified_config(rootcert: &std::path::Path) -> Result<ClientConfig, Error> {
    let pem = std::fs::read(rootcert).map_err(|source| Error::RootcertRead {
        path: rootcert.to_path_buf(),
        source,
    })?;
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<_, _>>()
        .map_err(|source| Error::RootcertParse {
            path: rootcert.to_path_buf(),
            source,
        })?;
    let mut roots = rustls::RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(certs);
    if added == 0 {
        return Err(Error::RootcertEmpty {
            path: rootcert.to_path_buf(),
        });
    }
    let provider = Arc::new(ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(config)
}

/// Connect, honouring every sslmode including `verify-full`. The connection
/// driver is spawned and its [`tokio::task::JoinHandle`] returned, so the
/// caller chooses between detaching (drop the handle, as `pg.rs` does) and
/// holding it to abort (as `er-entity-cli` does) — and so the two connector
/// branches' different stream types never escape this function.
///
/// # Errors
/// `Error` — planning failures are the crate's own; connect failures come
/// through `Error::Postgres`.
pub async fn connect(
    dsn: &str,
) -> Result<
    (
        tokio_postgres::Client,
        tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
    ),
    Error,
> {
    let plan = plan(dsn)?;
    if plan.mode == SslMode::VerifyFull {
        let rootcert = plan.sslrootcert.ok_or(Error::VerifyFullNeedsRootcert)?;
        let (client, connection) =
            tokio_postgres::connect(&plan.dsn, make_tls_verified(&rootcert)?).await?;
        return Ok((client, tokio::spawn(connection)));
    }
    let (client, connection) = tokio_postgres::connect(&plan.dsn, make_tls()).await?;
    Ok((client, tokio::spawn(connection)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_handshake_checks_trust_and_hostname() {
        let identity = rcgen::generate_simple_self_signed(vec!["db.example".into()]).unwrap();
        let unrelated = rcgen::generate_simple_self_signed(vec!["db.example".into()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let trusted = dir.path().join("trusted.pem");
        let wrong = dir.path().join("wrong.pem");
        std::fs::write(&trusted, identity.cert.pem()).unwrap();
        std::fs::write(&wrong, unrelated.cert.pem()).unwrap();
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(identity.signing_key.serialize_der());
        let server =
            rustls::ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![identity.cert.der().clone()], key.into())
                .unwrap();
        let handshake = |root: &std::path::Path, hostname: &'static str| {
            let mut client = rustls::ClientConnection::new(
                Arc::new(verified_config(root).unwrap()),
                ServerName::try_from(hostname).unwrap(),
            )
            .unwrap();
            let mut server = rustls::ServerConnection::new(Arc::new(server.clone())).unwrap();
            for _ in 0..10 {
                let mut bytes = Vec::new();
                client.write_tls(&mut bytes).unwrap();
                server.read_tls(&mut bytes.as_slice()).unwrap();
                server.process_new_packets()?;
                bytes.clear();
                server.write_tls(&mut bytes).unwrap();
                client.read_tls(&mut bytes.as_slice()).unwrap();
                client.process_new_packets()?;
                if !client.is_handshaking() && !server.is_handshaking() {
                    return Ok(());
                }
            }
            panic!("TLS handshake did not complete")
        };
        assert!(handshake(&trusted, "db.example").is_ok());
        assert!(matches!(
            handshake(&wrong, "db.example"),
            Err(rustls::Error::InvalidCertificate(_))
        ));
        assert!(matches!(
            handshake(&trusted, "wrong.example"),
            Err(rustls::Error::InvalidCertificate(_))
        ));
    }

    #[test]
    fn require_passes_through_untouched() {
        let plan = plan("postgresql://u:p@h:5432/er?sslmode=require").unwrap();
        assert_eq!(plan.mode, SslMode::Require);
        assert_eq!(plan.dsn, "postgresql://u:p@h:5432/er?sslmode=require");
        assert_eq!(plan.sslrootcert, None);
    }

    #[test]
    fn no_sslmode_stays_absent() {
        let plan = plan("postgresql://u:p@h/er").unwrap();
        assert_eq!(plan.mode, SslMode::Prefer);
        assert_eq!(plan.dsn, "postgresql://u:p@h/er");
    }

    #[test]
    fn verify_full_is_lifted_out_and_the_wire_stays_mandatory() {
        let plan =
            plan("postgresql://u:p@h:5432/er?sslmode=verify-full&sslrootcert=/etc/ssl/rds/global-bundle.pem")
                .unwrap();
        assert_eq!(plan.mode, SslMode::VerifyFull);
        assert_eq!(plan.dsn, "postgresql://u:p@h:5432/er?sslmode=require");
        assert_eq!(
            plan.sslrootcert,
            Some(PathBuf::from("/etc/ssl/rds/global-bundle.pem"))
        );
    }

    #[test]
    fn verify_full_preserves_unrelated_query_params() {
        let plan = plan(
            "postgresql://u:p@h/er?application_name=er&sslmode=verify-full&sslrootcert=/b.pem",
        )
        .unwrap();
        assert_eq!(
            plan.dsn,
            "postgresql://u:p@h/er?application_name=er&sslmode=require"
        );
    }

    #[test]
    fn query_values_keep_postgres_percent_encoding_semantics() {
        let planned = plan("postgresql://u:p%2Bss@h/er?application_name=hello%20world+plus&sslmode=verify-full&sslrootcert=/a+b.pem").unwrap();
        let config: tokio_postgres::Config = planned.dsn.parse().unwrap();
        assert_eq!(config.get_application_name(), Some("hello world+plus"));
        assert_eq!(config.get_password(), Some(b"p+ss".as_slice()));
        assert_eq!(planned.sslrootcert, Some(PathBuf::from("/a+b.pem")));
    }

    #[test]
    fn verify_ca_is_refused_loudly() {
        let err = plan("postgresql://u:p@h/er?sslmode=verify-ca&sslrootcert=/b.pem").unwrap_err();
        assert!(matches!(err, Error::UnsupportedSslMode(ref m) if m == "verify-ca"));
    }

    #[test]
    fn disable_is_left_for_tokio_postgres() {
        let plan = plan("postgresql://u:p@h/er?sslmode=disable").unwrap();
        assert_eq!(plan.mode, SslMode::Disable);
        assert_eq!(plan.dsn, "postgresql://u:p@h/er?sslmode=disable");
    }

    #[test]
    fn keyword_form_passes_through() {
        let plan = plan("host=h dbname=er sslmode=require").unwrap();
        assert_eq!(plan.dsn, "host=h dbname=er sslmode=require");
        assert_eq!(plan.mode, SslMode::Prefer);
    }

    #[test]
    fn verify_full_without_a_bundle_is_a_misconfiguration() {
        assert!(matches!(
            plan("postgresql://u:p@h/er?sslmode=verify-full"),
            Err(Error::VerifyFullNeedsRootcert)
        ));
        assert!(matches!(
            plan("postgresql://u:p@h/er?sslmode=verify-full&sslrootcert="),
            Err(Error::VerifyFullNeedsRootcert)
        ));
    }

    #[test]
    fn duplicate_tls_options_are_refused() {
        for query in [
            "sslmode=verify-full&sslmode=disable",
            "sslrootcert=/a&sslrootcert=/b",
        ] {
            assert!(matches!(
                plan(&format!("postgresql://h/er?{query}")),
                Err(Error::DuplicateTlsOption)
            ));
        }
    }

    #[test]
    fn a_bundle_that_reads_but_has_no_certs_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-cert.pem");
        std::fs::write(&path, b"no certificates here\n").unwrap();
        let err = make_tls_verified(&path).err().unwrap();
        assert!(matches!(err, Error::RootcertEmpty { .. }));
    }

    #[test]
    fn a_missing_bundle_names_the_path() {
        let err = make_tls_verified(std::path::Path::new("/nonexistent/bundle.pem"))
            .err()
            .unwrap();
        assert!(matches!(err, Error::RootcertRead { .. }));
        assert!(err.to_string().contains("/nonexistent/bundle.pem"));
    }
}
