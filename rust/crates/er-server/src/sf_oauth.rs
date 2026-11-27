//! Delegated Salesforce authorization for writeback (#994): the OAuth 2.0
//! web-server flow with PKCE, the identity binding, refresh and revocation.
//!
//! Writeback acts as a reviewing Salesforce user who consented, never as the
//! extraction's client credentials. Every network call takes an [`Egress`]:
//! product callers pass `Guarded`, so the instance URL is re-checked by the
//! outbound URL guard at use, and tests point it at a local mock.
//!
//! Tokens and verifiers are secrets. Nothing here logs one, and the errors
//! carry Salesforce's error CODE, never a token or a response body.

use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::urlguard::{self, Egress};

/// How long a started authorization may be completed.
pub const STATE_TTL_SECS: i64 = 10 * 60;

/// The scopes writeback needs: API access, and a refresh token so queued
/// work can run after the reviewer leaves.
pub const SCOPES: &str = "api refresh_token";

/// What the token endpoint returned.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub instance_url: String,
    /// The identity service URL (`…/id/<org id>/<user id>`).
    pub id_url: String,
    pub scope: String,
}

/// Who consented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub org_id: String,
    pub user_id: String,
    pub username: Option<String>,
}

/// Why a call to Salesforce did not succeed.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// The refresh token is no longer valid: consent was withdrawn, the user
    /// deactivated, or the token revoked. The grant must be marked revoked.
    #[error("Salesforce refused the grant ({0}); re-authorize")]
    Revoked(String),
    /// Salesforce refused the request for a reason re-trying will not fix.
    #[error("Salesforce refused the request ({0})")]
    Denied(String),
    /// Worth retrying: a network failure or a 5xx.
    #[error("Salesforce is unreachable or failing ({0})")]
    Transient(String),
    /// The response names a different Salesforce org or instance than the
    /// connection: refused rather than bound to the wrong tenant.
    #[error("the authorization belongs to a different Salesforce org")]
    OrgMismatch,
    #[error("the destination is refused")]
    Refused(#[from] urlguard::UrlRejection),
}

/// A random URL-safe token of `bytes` bytes.
fn random_token(bytes: usize) -> String {
    use rand::Rng;
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// A fresh `state` value and the SHA-256 the store keeps instead of it.
#[must_use]
pub fn new_state() -> (String, String) {
    let state = random_token(32);
    let hash = state_hash(&state);
    (state, hash)
}

/// The stored form of a state value.
#[must_use]
pub fn state_hash(state: &str) -> String {
    hex::encode(Sha256::digest(state.as_bytes()))
}

/// A PKCE verifier and its S256 challenge (RFC 7636).
#[must_use]
pub fn pkce() -> (String, String) {
    let verifier = random_token(48);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Where to send the reviewer to consent.
///
/// # Errors
/// An instance URL that does not parse.
pub fn authorize_url(
    instance_url: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Result<String, OAuthError> {
    let mut consent = reqwest::Url::parse(&format!(
        "{}/services/oauth2/authorize",
        instance_url.trim_end_matches('/')
    ))
    .map_err(|_| OAuthError::Refused(urlguard::UrlRejection::Unparseable))?;
    consent
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPES)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "consent");
    Ok(consent.to_string())
}

fn client() -> Result<reqwest::Client, OAuthError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        // Never follow a redirect: the guard checked THIS host, not wherever
        // a 30x points.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| OAuthError::Transient(format!("client: {e}")))
}

/// Classify a token-endpoint failure by Salesforce's `error` code.
fn classify(status: u16, body: &str) -> OAuthError {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| format!("http {status}"));
    if status >= 500 {
        OAuthError::Transient(code)
    } else if code == "invalid_grant" {
        OAuthError::Revoked(code)
    } else {
        OAuthError::Denied(code)
    }
}

fn same_instance(a: &str, b: &str) -> bool {
    let host = |u: &str| {
        reqwest::Url::parse(u)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
    };
    host(a).is_some() && host(a) == host(b)
}

async fn token_request(
    instance_url: &str,
    form: &[(&str, &str)],
    egress: Egress,
) -> Result<Tokens, OAuthError> {
    let url = format!(
        "{}/services/oauth2/token",
        instance_url.trim_end_matches('/')
    );
    urlguard::check(&url, egress).await?;
    let resp = client()?
        .post(&url)
        .form(form)
        .send()
        .await
        .map_err(|e| OAuthError::Transient(format!("token request: {}", e.without_url())))?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| OAuthError::Transient(format!("token response: {}", e.without_url())))?;
    if !(200..300).contains(&status) {
        return Err(classify(status, &body));
    }
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| OAuthError::Transient("token response is not JSON".into()))?;
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let tokens = Tokens {
        access_token: field("access_token")
            .ok_or_else(|| OAuthError::Denied("no access_token".into()))?,
        refresh_token: field("refresh_token"),
        instance_url: field("instance_url").unwrap_or_else(|| instance_url.to_string()),
        id_url: field("id").unwrap_or_default(),
        scope: field("scope").unwrap_or_default(),
    };
    // A code or refresh token minted for another instance is another tenant's.
    if !same_instance(&tokens.instance_url, instance_url) {
        return Err(OAuthError::OrgMismatch);
    }
    Ok(tokens)
}

/// Exchange an authorization code for tokens.
///
/// # Errors
/// See [`OAuthError`].
pub async fn exchange_code(
    instance_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    egress: Egress,
) -> Result<Tokens, OAuthError> {
    token_request(
        instance_url,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ],
        egress,
    )
    .await
}

/// A fresh access token from a stored refresh token.
///
/// # Errors
/// [`OAuthError::Revoked`] when consent is gone; see [`OAuthError`].
pub async fn refresh(
    instance_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
    egress: Egress,
) -> Result<Tokens, OAuthError> {
    token_request(
        instance_url,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ],
        egress,
    )
    .await
}

/// Who the tokens belong to, from the identity service. The org id must match
/// the one the id URL names — Salesforce's own binding of the two.
///
/// # Errors
/// See [`OAuthError`].
pub async fn identity(tokens: &Tokens, egress: Egress) -> Result<Identity, OAuthError> {
    let parts: Vec<&str> = tokens
        .id_url
        .trim_end_matches('/')
        .rsplit('/')
        .take(2)
        .collect();
    let [user_from_url, org_from_url] = parts[..] else {
        return Err(OAuthError::Denied("no identity URL".into()));
    };
    if !same_instance(&tokens.id_url, &tokens.instance_url) {
        return Err(OAuthError::OrgMismatch);
    }
    urlguard::check(&tokens.id_url, egress).await?;
    let resp = client()?
        .get(&tokens.id_url)
        .bearer_auth(&tokens.access_token)
        .send()
        .await
        .map_err(|e| OAuthError::Transient(format!("identity request: {}", e.without_url())))?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| OAuthError::Transient(format!("identity response: {}", e.without_url())))?;
    if !(200..300).contains(&status) {
        return Err(classify(status, &body));
    }
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| OAuthError::Transient("identity is not JSON".into()))?;
    let org_id = v
        .get("organization_id")
        .and_then(|x| x.as_str())
        .unwrap_or_default();
    let user_id = v
        .get("user_id")
        .and_then(|x| x.as_str())
        .unwrap_or_default();
    if org_id != org_from_url || user_id != user_from_url {
        return Err(OAuthError::OrgMismatch);
    }
    Ok(Identity {
        org_id: org_id.to_string(),
        user_id: user_id.to_string(),
        username: v
            .get("username")
            .and_then(|x| x.as_str())
            .map(str::to_string),
    })
}

/// Revoke a token at Salesforce. Best-effort by design: the local grant is
/// revoked regardless, so a failure here is reported, not fatal.
///
/// # Errors
/// See [`OAuthError`].
pub async fn revoke(instance_url: &str, token: &str, egress: Egress) -> Result<(), OAuthError> {
    let url = format!(
        "{}/services/oauth2/revoke",
        instance_url.trim_end_matches('/')
    );
    urlguard::check(&url, egress).await?;
    let resp = client()?
        .post(&url)
        .form(&[("token", token)])
        .send()
        .await
        .map_err(|e| OAuthError::Transient(format!("revoke request: {}", e.without_url())))?;
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(classify(status, &body))
    }
}

/// A usable delegated access token, and who it acts as.
#[derive(Debug, Clone)]
pub struct DelegatedAccess {
    pub access_token: String,
    pub instance_url: String,
    pub sf_org_id: String,
    pub sf_user_id: String,
    pub allow_automatic: bool,
}

/// Why no delegated access could be produced.
#[derive(Debug, thiserror::Error)]
pub enum AccessError {
    #[error("no active delegated authorization for this connection")]
    NotAuthorized,
    #[error("the delegated authorization does not allow unattended work")]
    NotForAutomatic,
    #[error("the connection no longer exists or is inactive")]
    NoConnection,
    #[error("delegated authorization failed")]
    OAuth(#[from] OAuthError),
    #[error("reading the authorization failed")]
    Db(#[from] tokio_postgres::Error),
}

/// A fresh access token for writeback through `connection_id`, acting as the
/// user who consented. `unattended` work (queued, policy-driven) is refused
/// unless the grant explicitly allows it. When Salesforce refuses the refresh
/// as invalid, the grant is marked revoked (`consent_withdrawn`) before the
/// error returns, so nothing retries a dead grant. The extraction's client
/// credentials are never a fallback.
///
/// # Errors
/// See [`AccessError`].
pub async fn access_token(
    client: &tokio_postgres::Client,
    org_id: &str,
    connection_id: &str,
    unattended: bool,
    egress: Egress,
) -> Result<DelegatedAccess, AccessError> {
    let conn = crate::pg::salesforce_connection_credential(client, org_id, connection_id)
        .await?
        .filter(|c| c.active)
        .ok_or(AccessError::NoConnection)?;
    let grant =
        er_entities::salesforce_delegated_grant::read::for_use(client, org_id, connection_id)
            .await?
            .into_iter()
            .next()
            .ok_or(AccessError::NotAuthorized)?;
    if unattended && !grant.allow_automatic {
        return Err(AccessError::NotForAutomatic);
    }
    match refresh(
        &conn.instance_url,
        &conn.client_id,
        &conn.client_secret,
        &grant.refresh_token,
        egress,
    )
    .await
    {
        Ok(tokens) => Ok(DelegatedAccess {
            access_token: tokens.access_token,
            instance_url: tokens.instance_url,
            sf_org_id: grant.sf_org_id,
            sf_user_id: grant.sf_user_id,
            allow_automatic: grant.allow_automatic,
        }),
        Err(OAuthError::Revoked(code)) => {
            er_entities::salesforce_delegated_grant::update::revoke(
                client,
                "consent_withdrawn",
                org_id,
                connection_id,
            )
            .await?;
            Err(AccessError::OAuth(OAuthError::Revoked(code)))
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        let (verifier, challenge) = pkce();
        assert!(verifier.len() >= 43, "RFC 7636 minimum");
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
        let (a, ha) = new_state();
        let (b, _) = new_state();
        assert_ne!(a, b);
        assert_eq!(ha, state_hash(&a));
        assert_ne!(ha, a, "the stored form is not the state");
    }

    #[test]
    fn the_authorize_url_carries_pkce_state_and_scopes() {
        let url = authorize_url(
            "https://x.my.salesforce.com/",
            "cid",
            "https://app/cb",
            "st",
            "ch",
        )
        .unwrap();
        assert!(url.starts_with("https://x.my.salesforce.com/services/oauth2/authorize?"));
        for part in [
            "response_type=code",
            "client_id=cid",
            "state=st",
            "code_challenge=ch",
            "code_challenge_method=S256",
            "scope=api+refresh_token",
        ] {
            assert!(url.contains(part), "{part} in {url}");
        }
    }

    #[test]
    fn token_failures_classify_by_salesforce_error_code() {
        assert!(matches!(
            classify(400, r#"{"error":"invalid_grant"}"#),
            OAuthError::Revoked(_)
        ));
        assert!(matches!(
            classify(400, r#"{"error":"invalid_client"}"#),
            OAuthError::Denied(_)
        ));
        assert!(matches!(classify(503, "down"), OAuthError::Transient(_)));
        // A body never leaks into the message beyond its error code.
        let e = classify(
            400,
            r#"{"error":"invalid_grant","error_description":"token abc123"}"#,
        );
        assert!(!e.to_string().contains("abc123"));
    }
}
