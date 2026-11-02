//! The projected ServiceAccount token credential for the attempt channel
//! (batch-orchestration-design §6.4): the task pod presents the
//! audience-bound token Kubernetes minted for it, and er-api verifies it
//! OFFLINE against the cluster's JSON Web Key Set — nothing secret is stored
//! anywhere, and the platform proves which pod is speaking.
//!
//! The key set comes from the in-cluster API server at the FIXED path
//! `https://kubernetes.default.svc/openid/v1/jwks`, never by following the
//! discovery document's `jwks_uri` (on EKS that names the public issuer's
//! throttled, rotating keys URL), authenticated with er-api's own projected
//! API-server-audience token. The cache refreshes lazily: on an unknown `kid`
//! (key rotation, at most once a minute) and after a timer. `iss` is
//! deliberately not checked — the keys came from the API server itself, so
//! there is no unverified issuer claim to trust.
//!
//! The random per-attempt fallback token (hash at rest on the row, `ct_eq`,
//! nulled at finish) remains accepted beside this: a pod whose template does
//! not mount the projected volume still reports. A bearer that PARSES as a
//! JWT is only ever verified as one — it never falls through to the hash
//! comparison, so a forged token cannot choose its weaker path.

use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use tokio::sync::Mutex;

/// The audience the task template's projected volume binds (§6.2).
pub const AUDIENCE: &str = "er-control-plane";
/// Bounded clock-skew tolerance on `exp`/`nbf` (§6.4).
const LEEWAY_SECS: u64 = 30;
/// Re-fetch the key set this often even with no rotation observed.
const REFRESH_AFTER: Duration = Duration::from_secs(3600);
/// An unknown `kid` triggers at most one fetch per this window.
const UNKNOWN_KID_FETCH_FLOOR: Duration = Duration::from_secs(60);

/// What the verified token proves: the `kubernetes.io` claims the server
/// records at `start` and the watch cross-checks later. Pod identity comes
/// from HERE, never from a request body (§6.3 step 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodClaims {
    pub namespace: String,
    pub pod_name: String,
    pub pod_uid: String,
    pub service_account: String,
}

/// Why a token was refused. One public string per case — the handler maps all
/// of them to 401 without detail, and the log line carries this.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("the token header does not parse")]
    Header(#[source] jsonwebtoken::errors::Error),
    #[error("the token names no `kid`")]
    NoKid,
    #[error("algorithm {0:?} is not RS256/ES256")]
    Algorithm(Algorithm),
    #[error("no key `{kid}` in the cluster key set")]
    UnknownKid { kid: String },
    #[error("the key set could not be fetched: {0}")]
    Fetch(String),
    #[error("the signature, audience or validity window failed")]
    Invalid(#[source] jsonwebtoken::errors::Error),
    #[error("the token carries no pod claim (not a pod-bound token)")]
    NoPod,
}

/// A boxed future, spelled with std types so the trait needs no `futures`
/// dependency edge.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where the JWKS JSON comes from: the in-cluster API server in production,
/// a closure in tests. One method, so the double cannot drift.
pub trait JwksSource: Send + Sync {
    fn fetch(&self) -> BoxFuture<'_, Result<String, String>>;
}

/// The production source: `https://kubernetes.default.svc/openid/v1/jwks`
/// with the pod's own projected token and the cluster CA.
struct InCluster {
    http: reqwest::Client,
    token_path: std::path::PathBuf,
}

const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
const JWKS_URL: &str = "https://kubernetes.default.svc/openid/v1/jwks";

impl JwksSource for InCluster {
    fn fetch(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            // Re-read per fetch: projected tokens rotate.
            let token = std::fs::read_to_string(&self.token_path)
                .map_err(|e| format!("reading the serviceaccount token: {e}"))?;
            let resp = self
                .http
                .get(JWKS_URL)
                .bearer_auth(token.trim())
                .send()
                .await
                .map_err(|e| format!("GET {JWKS_URL}: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("GET {JWKS_URL}: HTTP {}", resp.status()));
            }
            resp.text()
                .await
                .map_err(|e| format!("reading the key set body: {e}"))
        })
    }
}

struct Cache {
    /// `kid` → decoding key, from the last good fetch.
    keys: Vec<(String, DecodingKey, Algorithm)>,
    fetched_at: Option<Instant>,
}

/// The verifier: a lazily-refreshed key cache over one [`JwksSource`].
pub struct Verifier {
    source: Box<dyn JwksSource>,
    cache: Mutex<Cache>,
}

impl Verifier {
    #[must_use]
    pub fn new(source: Box<dyn JwksSource>) -> Self {
        Verifier {
            source,
            cache: Mutex::new(Cache {
                keys: Vec::new(),
                fetched_at: None,
            }),
        }
    }

    /// The in-cluster verifier, or `None` with the reason logged when this
    /// process is not a pod (no projected token / CA at the default path).
    /// Local and test processes run fallback-token-only, exactly as before.
    #[must_use]
    pub fn in_cluster() -> Option<Self> {
        let dir = std::path::Path::new(SA_DIR);
        let token_path = dir.join("token");
        let ca_path = dir.join("ca.crt");
        if !token_path.exists() {
            return None;
        }
        let ca = match std::fs::read(&ca_path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("satoken: no cluster CA at {}: {e}", ca_path.display());
                return None;
            }
        };
        let cert = match reqwest::Certificate::from_pem(&ca) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("satoken: the cluster CA does not parse: {e}");
                return None;
            }
        };
        let http = match reqwest::Client::builder()
            .add_root_certificate(cert)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("satoken: building the JWKS client failed: {e}");
                return None;
            }
        };
        Some(Verifier::new(Box::new(InCluster { http, token_path })))
    }

    async fn refresh(&self, cache: &mut Cache) -> Result<(), VerifyError> {
        let body = self.source.fetch().await.map_err(VerifyError::Fetch)?;
        let set: JwkSet =
            serde_json::from_str(&body).map_err(|e| VerifyError::Fetch(e.to_string()))?;
        let mut keys = Vec::new();
        for jwk in &set.keys {
            let Some(kid) = jwk.common.key_id.clone() else {
                continue;
            };
            let Ok(key) = DecodingKey::from_jwk(jwk) else {
                continue;
            };
            let alg = match &jwk.algorithm {
                jsonwebtoken::jwk::AlgorithmParameters::RSA(_) => Algorithm::RS256,
                jsonwebtoken::jwk::AlgorithmParameters::EllipticCurve(_) => Algorithm::ES256,
                _ => continue,
            };
            keys.push((kid, key, alg));
        }
        cache.keys = keys;
        cache.fetched_at = Some(Instant::now());
        Ok(())
    }

    /// Verify one bearer: signature against the cluster keys, `aud`,
    /// `exp`/`nbf` with the bounded leeway — and return the pod claims.
    ///
    /// # Errors
    /// Any refusal, each naming its cause for the log; the handler answers a
    /// uniform 401.
    pub async fn verify(&self, token: &str) -> Result<PodClaims, VerifyError> {
        let header = decode_header(token).map_err(VerifyError::Header)?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(VerifyError::Algorithm(header.alg));
        }
        let kid = header.kid.ok_or(VerifyError::NoKid)?;

        let mut cache = self.cache.lock().await;
        let stale = cache
            .fetched_at
            .is_none_or(|at| at.elapsed() >= REFRESH_AFTER);
        if stale {
            self.refresh(&mut cache).await?;
        }
        if !cache.keys.iter().any(|(k, _, _)| *k == kid) {
            // Rotation: one fetch, floored so a stream of bad kids cannot
            // hammer the API server.
            let recently = cache
                .fetched_at
                .is_some_and(|at| at.elapsed() < UNKNOWN_KID_FETCH_FLOOR);
            if !recently {
                self.refresh(&mut cache).await?;
            }
        }
        let Some((_, key, alg)) = cache.keys.iter().find(|(k, _, _)| *k == kid) else {
            return Err(VerifyError::UnknownKid { kid });
        };

        let mut validation = Validation::new(*alg);
        validation.set_audience(&[AUDIENCE]);
        validation.leeway = LEEWAY_SECS;
        validation.validate_nbf = true;
        // The keys came from the API server itself; there is no issuer claim
        // worth trusting beyond that (§6.4).
        validation.validate_aud = true;

        let data = decode::<TokenClaims>(token, key, &validation).map_err(VerifyError::Invalid)?;
        let k8s = data.claims.kubernetes;
        let pod = k8s.pod.ok_or(VerifyError::NoPod)?;
        Ok(PodClaims {
            namespace: k8s.namespace,
            pod_name: pod.name,
            pod_uid: pod.uid,
            service_account: k8s.serviceaccount.map(|s| s.name).unwrap_or_default(),
        })
    }
}

/// Whether a bearer is SHAPED like a JWT (three dot-separated segments whose
/// header parses). A JWT-shaped bearer is only ever verified as one — never
/// hashed against the fallback column.
#[must_use]
pub fn looks_like_jwt(token: &str) -> bool {
    token.bytes().filter(|b| *b == b'.').count() == 2 && decode_header(token).is_ok()
}

#[derive(Deserialize)]
struct TokenClaims {
    // `exp`/`nbf`/`aud` are validated by jsonwebtoken itself and need no
    // fields here; only the platform's claims are read out.
    #[serde(rename = "kubernetes.io")]
    kubernetes: K8sClaims,
}

#[derive(Deserialize)]
struct K8sClaims {
    namespace: String,
    #[serde(default)]
    pod: Option<RefClaim>,
    #[serde(default)]
    serviceaccount: Option<RefClaim>,
}

#[derive(Deserialize)]
struct RefClaim {
    name: String,
    #[serde(default)]
    uid: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TEST_PEM: &str = include_str!("../testdata/sa_token_test_rsa.pem");
    const TEST_JWKS: &str = include_str!("../testdata/sa_token_test_jwks.json");

    struct Fixed {
        body: String,
        fetches: AtomicUsize,
    }

    impl JwksSource for Fixed {
        fn fetch(&self) -> BoxFuture<'_, Result<String, String>> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            let body = self.body.clone();
            Box::pin(async move { Ok(body) })
        }
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn mint(kid: &str, aud: &str, exp_offset: i64, pod: Option<(&str, &str)>) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let mut k8s = json!({ "namespace": "er-dev-test" });
        if let Some((name, uid)) = pod {
            k8s["pod"] = json!({ "name": name, "uid": uid });
            k8s["serviceaccount"] = json!({ "name": "er-runner", "uid": "sa-uid" });
        }
        let claims = json!({
            "aud": [aud],
            "exp": now().saturating_add_signed(exp_offset),
            "iat": now() - 10,
            "nbf": now() - 10,
            "sub": "system:serviceaccount:er-dev-test:er-runner",
            "kubernetes.io": k8s,
        });
        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(TEST_PEM.as_bytes()).expect("test key"),
        )
        .expect("mint")
    }

    fn verifier() -> Verifier {
        Verifier::new(Box::new(Fixed {
            body: TEST_JWKS.to_string(),
            fetches: AtomicUsize::new(0),
        }))
    }

    #[tokio::test]
    async fn a_good_pod_token_yields_its_claims() {
        let token = mint(
            "test-kid-1",
            AUDIENCE,
            600,
            Some(("er-t-ab-l1-x9z2k", "uid-1")),
        );
        let claims = verifier().verify(&token).await.expect("verifies");
        assert_eq!(claims.pod_name, "er-t-ab-l1-x9z2k");
        assert_eq!(claims.pod_uid, "uid-1");
        assert_eq!(claims.namespace, "er-dev-test");
        assert_eq!(claims.service_account, "er-runner");
    }

    #[tokio::test]
    async fn the_wrong_audience_is_refused() {
        let token = mint(
            "test-kid-1",
            "kubernetes.default.svc",
            600,
            Some(("p", "u")),
        );
        let err = verifier().verify(&token).await.expect_err("refused");
        assert!(matches!(err, VerifyError::Invalid(_)), "{err}");
    }

    #[tokio::test]
    async fn an_expired_token_is_refused_beyond_the_leeway() {
        let token = mint("test-kid-1", AUDIENCE, -120, Some(("p", "u")));
        let err = verifier().verify(&token).await.expect_err("refused");
        assert!(matches!(err, VerifyError::Invalid(_)), "{err}");
        // Inside the 30 s leeway it still verifies (bounded skew, §6.4).
        let token = mint("test-kid-1", AUDIENCE, -10, Some(("p", "u")));
        verifier().verify(&token).await.expect("within leeway");
    }

    #[tokio::test]
    async fn a_token_without_a_pod_claim_is_refused() {
        let token = mint("test-kid-1", AUDIENCE, 600, None);
        let err = verifier().verify(&token).await.expect_err("refused");
        assert!(matches!(err, VerifyError::NoPod), "{err}");
    }

    #[tokio::test]
    async fn an_unknown_kid_refreshes_once_then_refuses() {
        let source = Fixed {
            body: TEST_JWKS.to_string(),
            fetches: AtomicUsize::new(0),
        };
        let v = Verifier::new(Box::new(source));
        let token = mint("rotated-away", AUDIENCE, 600, Some(("p", "u")));
        let err = v.verify(&token).await.expect_err("refused");
        assert!(matches!(err, VerifyError::UnknownKid { .. }), "{err}");
        // The empty cache fetched once; the unknown kid did NOT trigger a
        // second fetch inside the floor window.
        let good = mint("test-kid-1", AUDIENCE, 600, Some(("p", "u")));
        v.verify(&good).await.expect("the fetched key set serves");
    }

    #[tokio::test]
    async fn a_tampered_signature_is_refused() {
        let minted = mint("test-kid-1", AUDIENCE, 600, Some(("p", "u")));
        // Splice a different payload between the genuine header and signature.
        let [header, _, sig]: [&str; 3] = minted
            .split('.')
            .collect::<Vec<_>>()
            .try_into()
            .expect("three segments");
        let other = mint("test-kid-1", AUDIENCE, 600, Some(("intruder", "u2")));
        let payload = other.split('.').nth(1).unwrap();
        let token = format!("{header}.{payload}.{sig}");
        let err = verifier().verify(&token).await.expect_err("refused");
        assert!(matches!(err, VerifyError::Invalid(_)), "{err}");
    }

    #[test]
    fn jwt_shape_detection() {
        assert!(looks_like_jwt(&mint("k", AUDIENCE, 600, Some(("p", "u")))));
        assert!(!looks_like_jwt("a1b2c3d4"));
        assert!(!looks_like_jwt("a.b.c"));
    }
}
