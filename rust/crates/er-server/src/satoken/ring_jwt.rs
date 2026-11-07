//! A ring-backed [`CryptoProvider`] for `jsonwebtoken`.
//!
//! jsonwebtoken 10+ ships no crypto of its own: a process has to pick a
//! backend, and the two built-in ones are both closed to this image —
//! `aws_lc_rs` is banned (its `-sys` crate needs cmake the build image lacks,
//! and rustls/ring is the one crypto stack linked here), and `rust_crypto`
//! pulls the `rsa` crate, whose unfixed timing advisories fail cargo-deny.
//! ring is already in the graph through rustls, so this provider adds no
//! crate.
//!
//! Scope is what the SA-token path uses: RSA (PKCS#1 v1.5 and PSS) and ECDSA
//! P-256/P-384, sign and verify. HMAC and EdDSA are refused, and so are the
//! JWK-from-private-key helpers in [`KeyUtils`] — nothing here builds a JWK
//! from a key, only a key from a JWK. Every refusal is an error, never a
//! panic.

use jsonwebtoken::crypto::{CryptoProvider, JwtSigner, JwtVerifier, KeyUtils};
use jsonwebtoken::errors::{Error, ErrorKind, Result};
use jsonwebtoken::jwk::ThumbprintHash;
use jsonwebtoken::signature::{self, Signer, Verifier};
use jsonwebtoken::{Algorithm, AlgorithmFamily, DecodingKey, DecodingKeyKind, EncodingKey};
use ring::rand::SystemRandom;
use ring::signature as sig;

// ring's `Unspecified` is deliberately information-free (and is not a
// `std::error::Error` without ring's `std` feature), so failures map to the
// opaque `signature::Error::new()`; jsonwebtoken reports them as
// `InvalidSignature` either way.

/// Install the provider as the process default. Idempotent; must run before
/// the first `encode`/`decode`, because jsonwebtoken latches its default on
/// first use.
pub fn install() {
    // `Err` only means a provider is already installed — ours, since this
    // crate is the only jsonwebtoken user in the workspace.
    let _ = PROVIDER.install_default();
}

static PROVIDER: CryptoProvider = CryptoProvider {
    signer_factory: new_signer,
    verifier_factory: new_verifier,
    key_utils: KeyUtils {
        rsa_pub_components_from_private_key: |_| Err(ErrorKind::UnsupportedAlgorithm.into()),
        rsa_pub_components_from_public_key: |_| Err(ErrorKind::UnsupportedAlgorithm.into()),
        ec_pub_components_from_private_key: |_, _| Err(ErrorKind::UnsupportedAlgorithm.into()),
        ed_pub_components_from_private_key: |_, _| Err(ErrorKind::UnsupportedAlgorithm.into()),
        compute_digest,
    },
};

fn compute_digest(data: &[u8], hash: ThumbprintHash) -> Result<Vec<u8>> {
    let alg = match hash {
        ThumbprintHash::SHA256 => &ring::digest::SHA256,
        ThumbprintHash::SHA384 => &ring::digest::SHA384,
        ThumbprintHash::SHA512 => &ring::digest::SHA512,
        _ => return Err(ErrorKind::UnsupportedAlgorithm.into()),
    };
    Ok(ring::digest::digest(alg, data).as_ref().to_vec())
}

fn new_signer(alg: &Algorithm, key: &EncodingKey) -> Result<Box<dyn JwtSigner>> {
    let signer: Box<dyn JwtSigner> = match alg {
        Algorithm::RS256 => RsaSigner::new(*alg, &sig::RSA_PKCS1_SHA256, key)?,
        Algorithm::RS384 => RsaSigner::new(*alg, &sig::RSA_PKCS1_SHA384, key)?,
        Algorithm::RS512 => RsaSigner::new(*alg, &sig::RSA_PKCS1_SHA512, key)?,
        Algorithm::PS256 => RsaSigner::new(*alg, &sig::RSA_PSS_SHA256, key)?,
        Algorithm::PS384 => RsaSigner::new(*alg, &sig::RSA_PSS_SHA384, key)?,
        Algorithm::PS512 => RsaSigner::new(*alg, &sig::RSA_PSS_SHA512, key)?,
        Algorithm::ES256 => EcSigner::new(*alg, &sig::ECDSA_P256_SHA256_FIXED_SIGNING, key)?,
        Algorithm::ES384 => EcSigner::new(*alg, &sig::ECDSA_P384_SHA384_FIXED_SIGNING, key)?,
        _ => return Err(ErrorKind::UnsupportedAlgorithm.into()),
    };
    Ok(signer)
}

fn new_verifier(alg: &Algorithm, key: &DecodingKey) -> Result<Box<dyn JwtVerifier>> {
    let verifier = match alg {
        Algorithm::RS256 => RsaVerifier::new(*alg, &sig::RSA_PKCS1_2048_8192_SHA256, key)?,
        Algorithm::RS384 => RsaVerifier::new(*alg, &sig::RSA_PKCS1_2048_8192_SHA384, key)?,
        Algorithm::RS512 => RsaVerifier::new(*alg, &sig::RSA_PKCS1_2048_8192_SHA512, key)?,
        Algorithm::PS256 => RsaVerifier::new(*alg, &sig::RSA_PSS_2048_8192_SHA256, key)?,
        Algorithm::PS384 => RsaVerifier::new(*alg, &sig::RSA_PSS_2048_8192_SHA384, key)?,
        Algorithm::PS512 => RsaVerifier::new(*alg, &sig::RSA_PSS_2048_8192_SHA512, key)?,
        Algorithm::ES256 => EcVerifier::new(*alg, &sig::ECDSA_P256_SHA256_FIXED, key)?,
        Algorithm::ES384 => EcVerifier::new(*alg, &sig::ECDSA_P384_SHA384_FIXED, key)?,
        _ => return Err(ErrorKind::UnsupportedAlgorithm.into()),
    };
    Ok(verifier)
}

fn require_family(actual: AlgorithmFamily, want: AlgorithmFamily) -> Result<()> {
    if actual == want {
        Ok(())
    } else {
        Err(ErrorKind::InvalidKeyFormat.into())
    }
}

struct RsaSigner {
    alg: Algorithm,
    padding: &'static dyn sig::RsaEncoding,
    key: sig::RsaKeyPair,
}

impl RsaSigner {
    fn new(
        alg: Algorithm,
        padding: &'static dyn sig::RsaEncoding,
        key: &EncodingKey,
    ) -> Result<Box<dyn JwtSigner>> {
        require_family(key.family(), AlgorithmFamily::Rsa)?;
        let key = sig::RsaKeyPair::from_der(key.as_bytes())
            .map_err(|e| Error::from(ErrorKind::InvalidRsaKey(e.to_string())))?;
        Ok(Box::new(Self { alg, padding, key }))
    }
}

impl Signer<Vec<u8>> for RsaSigner {
    fn try_sign(&self, msg: &[u8]) -> std::result::Result<Vec<u8>, signature::Error> {
        let mut out = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(self.padding, &SystemRandom::new(), msg, &mut out)
            .map_err(|_| signature::Error::new())?;
        Ok(out)
    }
}

impl JwtSigner for RsaSigner {
    fn algorithm(&self) -> Algorithm {
        self.alg
    }
}

struct EcSigner {
    alg: Algorithm,
    key: sig::EcdsaKeyPair,
}

impl EcSigner {
    fn new(
        alg: Algorithm,
        signing: &'static sig::EcdsaSigningAlgorithm,
        key: &EncodingKey,
    ) -> Result<Box<dyn JwtSigner>> {
        require_family(key.family(), AlgorithmFamily::Ec)?;
        let key = sig::EcdsaKeyPair::from_pkcs8(signing, key.as_bytes(), &SystemRandom::new())
            .map_err(|_| Error::from(ErrorKind::InvalidEcdsaKey))?;
        Ok(Box::new(Self { alg, key }))
    }
}

impl Signer<Vec<u8>> for EcSigner {
    fn try_sign(&self, msg: &[u8]) -> std::result::Result<Vec<u8>, signature::Error> {
        let out = self
            .key
            .sign(&SystemRandom::new(), msg)
            .map_err(|_| signature::Error::new())?;
        Ok(out.as_ref().to_vec())
    }
}

impl JwtSigner for EcSigner {
    fn algorithm(&self) -> Algorithm {
        self.alg
    }
}

struct RsaVerifier {
    alg: Algorithm,
    params: &'static sig::RsaParameters,
    key: DecodingKey,
}

impl RsaVerifier {
    fn new(
        alg: Algorithm,
        params: &'static sig::RsaParameters,
        key: &DecodingKey,
    ) -> Result<Box<dyn JwtVerifier>> {
        require_family(key.family(), AlgorithmFamily::Rsa)?;
        Ok(Box::new(Self {
            alg,
            params,
            key: key.clone(),
        }))
    }
}

impl Verifier<Vec<u8>> for RsaVerifier {
    fn verify(&self, msg: &[u8], signature: &Vec<u8>) -> std::result::Result<(), signature::Error> {
        match self.key.kind() {
            DecodingKeyKind::SecretOrDer(der) => {
                sig::UnparsedPublicKey::new(self.params, der).verify(msg, signature)
            }
            DecodingKeyKind::RsaModulusExponent { n, e } => {
                sig::RsaPublicKeyComponents { n, e }.verify(self.params, msg, signature)
            }
        }
        .map_err(|_| signature::Error::new())
    }
}

impl JwtVerifier for RsaVerifier {
    fn algorithm(&self) -> Algorithm {
        self.alg
    }
}

struct EcVerifier {
    alg: Algorithm,
    params: &'static sig::EcdsaVerificationAlgorithm,
    key: DecodingKey,
}

impl EcVerifier {
    fn new(
        alg: Algorithm,
        params: &'static sig::EcdsaVerificationAlgorithm,
        key: &DecodingKey,
    ) -> Result<Box<dyn JwtVerifier>> {
        require_family(key.family(), AlgorithmFamily::Ec)?;
        Ok(Box::new(Self {
            alg,
            params,
            key: key.clone(),
        }))
    }
}

impl Verifier<Vec<u8>> for EcVerifier {
    fn verify(&self, msg: &[u8], signature: &Vec<u8>) -> std::result::Result<(), signature::Error> {
        // A JWK EC key decodes to the uncompressed SEC1 point, which is
        // exactly what ring's ECDSA verifier takes.
        let point = self
            .key
            .try_get_as_bytes()
            .map_err(signature::Error::from_source)?;
        sig::UnparsedPublicKey::new(self.params, point)
            .verify(msg, signature)
            .map_err(|_| signature::Error::new())
    }
}

impl JwtVerifier for EcVerifier {
    fn algorithm(&self) -> Algorithm {
        self.alg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Header, Validation, decode, encode};
    use ring::signature::KeyPair;
    use serde_json::{Value, json};

    /// ES256 end to end through the installed provider: a fresh P-256 key
    /// signs, the matching public point verifies, a different key refuses.
    #[test]
    fn es256_round_trips_and_refuses_a_foreign_key() {
        install();
        let rng = SystemRandom::new();
        let keypair = |pkcs8: &[u8]| {
            sig::EcdsaKeyPair::from_pkcs8(&sig::ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &rng)
                .expect("generated key parses")
        };
        let pkcs8 = sig::EcdsaKeyPair::generate_pkcs8(&sig::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .expect("generate");
        let other = sig::EcdsaKeyPair::generate_pkcs8(&sig::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .expect("generate");

        let claims = json!({ "sub": "s", "exp": jsonwebtoken::get_current_timestamp() + 600 });
        let token = encode(
            &Header::new(Algorithm::ES256),
            &claims,
            &EncodingKey::from_ec_der(pkcs8.as_ref()),
        )
        .expect("sign");

        let mut validation = Validation::new(Algorithm::ES256);
        validation.required_spec_claims.clear();
        let mine = DecodingKey::from_ec_der(keypair(pkcs8.as_ref()).public_key().as_ref());
        let data = decode::<Value>(&token, &mine, &validation).expect("verifies");
        assert_eq!(data.claims["sub"], "s");

        let theirs = DecodingKey::from_ec_der(keypair(other.as_ref()).public_key().as_ref());
        let err = decode::<Value>(&token, &theirs, &validation).expect_err("foreign key");
        assert!(matches!(err.kind(), ErrorKind::InvalidSignature), "{err:?}");
    }

    /// HMAC is out of scope and must refuse with an error, not panic.
    #[test]
    fn an_unsupported_family_is_an_error() {
        install();
        let err = encode(
            &Header::new(Algorithm::HS256),
            &json!({}),
            &EncodingKey::from_secret(b"k"),
        )
        .expect_err("refused");
        assert!(
            matches!(err.kind(), ErrorKind::UnsupportedAlgorithm),
            "{err:?}"
        );
    }
}
