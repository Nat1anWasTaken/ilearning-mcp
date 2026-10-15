//! `private_key_jwt` client authentication (RFC 7523 §2.2): the client signs a
//! short-lived JWT with a key it publishes at its `jwks_uri`. ChatGPT
//! authenticates this way.
//!
//! Key sets and seen assertion IDs are cached in memory, which assumes a
//! single server instance.

use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use dashmap::DashMap;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use url::Url;

use super::OAuthError;
use crate::net;

/// The `client_assertion_type` of a JWT client assertion (RFC 7523 §2.2).
pub const ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

/// Accepted signing algorithms, advertised in the server metadata.
pub const SIGNING_ALGORITHMS: [Algorithm; 3] =
    [Algorithm::RS256, Algorithm::PS256, Algorithm::ES256];

/// How long a fetched key set is used before it is fetched again.
const KEYS_MAX_AGE: Duration = Duration::from_secs(60 * 60);
/// An assertion signed with an unknown key triggers a refetch at most this often.
const REFETCH_INTERVAL: Duration = Duration::from_secs(60);
/// Assertions must expire within this many seconds of now (RFC 7523 §3, item 4).
const MAX_LIFETIME_SECS: u64 = 10 * 60;
const MAX_KEY_SET_BYTES: usize = 64 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct ClientAssertions {
    key_sets: DashMap<String, KeySet>,
    /// `(client_id, jti)` of accepted assertions, with their expiry, so none is
    /// accepted twice.
    seen: Mutex<HashMap<(String, String), u64>>,
}

#[derive(Clone)]
struct KeySet {
    keys: Arc<JwkSet>,
    fetched: Instant,
}

#[derive(Deserialize)]
struct Claims {
    exp: u64,
    jti: String,
}

impl ClientAssertions {
    /// Checks a client assertion: signed by one of the client's keys, issued
    /// by and about `client_id`, addressed to one of `audiences`, unexpired,
    /// short-lived and never seen before.
    pub async fn verify(
        &self,
        client_id: &str,
        jwks_uri: &str,
        assertion: &str,
        audiences: &[String],
    ) -> Result<(), OAuthError> {
        let header = decode_header(assertion).map_err(|_| rejected("is not a JWT"))?;
        if !SIGNING_ALGORITHMS.contains(&header.alg) {
            return Err(rejected("uses an unsupported signing algorithm"));
        }
        let key = self
            .key(jwks_uri, header.kid.as_deref(), header.alg)
            .await?;

        let mut validation = Validation::new(header.alg);
        validation.set_required_spec_claims(&["exp", "iss", "sub", "aud"]);
        validation.set_issuer(&[client_id]);
        validation.set_audience(audiences);
        validation.sub = Some(client_id.to_owned());
        let claims = decode::<Claims>(assertion, &key, &validation)
            .map_err(|err| rejected(&format!("is invalid ({err})")))?
            .claims;

        let now = jsonwebtoken::get_current_timestamp();
        if claims.exp > now + MAX_LIFETIME_SECS + validation.leeway {
            return Err(rejected("expires too far in the future"));
        }
        self.remember(client_id, claims.jti, claims.exp, now)
    }

    /// The key that verifies `alg` signatures with the given key ID, fetching
    /// the key set when it is stale or lacks the key.
    async fn key(
        &self,
        jwks_uri: &str,
        kid: Option<&str>,
        alg: Algorithm,
    ) -> Result<DecodingKey, OAuthError> {
        let cached = self.key_sets.get(jwks_uri).map(|entry| entry.clone());
        if let Some(set) = &cached {
            let fresh = set.fetched.elapsed() < KEYS_MAX_AGE;
            if let Some(key) = fresh.then(|| find_key(&set.keys, kid, alg)).flatten() {
                return Ok(key);
            }
            if fresh && set.fetched.elapsed() < REFETCH_INTERVAL {
                return Err(rejected("is signed with an unknown key"));
            }
        }
        let keys = Arc::new(fetch_key_set(jwks_uri).await?);
        self.key_sets.insert(
            jwks_uri.to_owned(),
            KeySet {
                keys: keys.clone(),
                fetched: Instant::now(),
            },
        );
        find_key(&keys, kid, alg).ok_or_else(|| rejected("is signed with an unknown key"))
    }

    fn remember(&self, client_id: &str, jti: String, exp: u64, now: u64) -> Result<(), OAuthError> {
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        seen.retain(|_, expires| *expires >= now);
        match seen.entry((client_id.to_owned(), jti)) {
            Entry::Occupied(_) => Err(rejected("was already used")),
            Entry::Vacant(entry) => {
                entry.insert(exp);
                Ok(())
            }
        }
    }
}

/// Matches by key ID when the assertion names one; otherwise the first key
/// that can verify `alg`.
fn find_key(keys: &JwkSet, kid: Option<&str>, alg: Algorithm) -> Option<DecodingKey> {
    keys.keys
        .iter()
        .filter(|jwk| kid.is_none_or(|kid| jwk.common.key_id.as_deref() == Some(kid)))
        .filter_map(|jwk| DecodingKey::from_jwk(jwk).ok())
        .find(|key| key.family() == alg.family())
}

async fn fetch_key_set(jwks_uri: &str) -> Result<JwkSet, OAuthError> {
    let url = Url::parse(jwks_uri).map_err(|_| rejected("names an invalid jwks_uri"))?;
    let fetched = net::fetch_public_url(&url, MAX_KEY_SET_BYTES, FETCH_TIMEOUT)
        .await
        .map_err(|err| {
            tracing::warn!(jwks_uri, error = %err, "fetching client keys failed");
            rejected("could not be checked: the client's keys are unavailable")
        })?;
    serde_json::from_slice(&fetched.body)
        .map_err(|_| rejected("could not be checked: the client's key set is invalid"))
}

fn rejected(reason: &str) -> OAuthError {
    OAuthError::invalid_client_with(format!("client assertion {reason}"))
}

/// A test client with a known key pair, shared with the OAuth integration tests.
#[cfg(test)]
pub(super) mod test_client {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::{Value, json};

    use super::*;

    // A throwaway P-256 key pair for tests only.
    const PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgoyKbHqvfO5PuiG0g
Os9IKlRRJClWPdry6TLXpMU/iRyhRANCAAR3hN/J5OmCo/0Xb8AGigvuiqnusE/D
uymr3UKEabdzWPVYFZrQHqCynokd0ERJQuJzAlgFKcQmM3uWFMfkxu6/
-----END PRIVATE KEY-----";
    const KID: &str = "test-key";
    pub const CLIENT_ID: &str = "https://client.example/oauth/client.json";
    pub const JWKS_URI: &str = "https://client.example/oauth/jwks.json";

    impl ClientAssertions {
        /// Caches the test client's key set, as if fetched from [`JWKS_URI`].
        pub fn preload_test_keys(&self) {
            let keys = serde_json::from_value(json!({ "keys": [{
                "kty": "EC", "crv": "P-256", "kid": KID, "use": "sig", "alg": "ES256",
                "x": "d4TfyeTpgqP9F2_ABooL7oqp7rBPw7spq91ChGm3c1g",
                "y": "9VgVmtAeoLKeiR3QRElC4nMCWAUpxCYze5YUx-TG7r8",
            }]}))
            .expect("valid key set");
            self.key_sets.insert(
                JWKS_URI.to_owned(),
                KeySet {
                    keys: Arc::new(keys),
                    fetched: Instant::now(),
                },
            );
        }
    }

    /// Claims of a valid assertion for `audience`.
    pub fn claims(audience: &str, jti: &str) -> Value {
        let now = jsonwebtoken::get_current_timestamp();
        json!({ "iss": CLIENT_ID, "sub": CLIENT_ID, "aud": audience, "exp": now + 300, "jti": jti })
    }

    pub fn sign(claims: &Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(KID.to_owned());
        let der: String = PRIVATE_KEY
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let key = EncodingKey::from_ec_der(&STANDARD.decode(der).expect("base64 key"));
        encode(&header, claims, &key).expect("signable claims")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        test_client::{CLIENT_ID as CLIENT, JWKS_URI, claims as claims_for, sign},
        *,
    };

    const AUDIENCE: &str = "https://mcp.example.test/oauth/token";

    fn verifier() -> ClientAssertions {
        let assertions = ClientAssertions::default();
        assertions.preload_test_keys();
        assertions
    }

    fn claims(jti: &str) -> serde_json::Value {
        claims_for(AUDIENCE, jti)
    }

    async fn check(verifier: &ClientAssertions, token: &str) -> Result<(), OAuthError> {
        verifier
            .verify(CLIENT, JWKS_URI, token, &[AUDIENCE.to_owned()])
            .await
    }

    /// ChatGPT's published key set, as of October 2026.
    #[test]
    fn finds_rsa_keys_in_chatgpts_key_set() {
        let keys: JwkSet = serde_json::from_value(json!({"keys": [{"kty": "RSA", "kid": "cimd-20260428030119", "use": "sig", "alg": "RS256", "n": "y09nMyYX6LhSgS3YmbLOZrFoR8SffxG0kM5gQ5PKpHVMzbAu__-7rf0_Q_pwhVa9vxJzv3cRkGnXKNWKOHDdEXp8YFPVUql4NcDjDdS_0w0uos8gazoa7Td47qVquxOsG3861l8oKEh-E4r5C_6w6Sx0Rl2WEEs2-dmvn3fwH9PkLCQOo4tsNAEnrW_ge2vQE-pFo-kJp5QRRiX2w0YvaMFtIfEvNbdPSJ3xd7NbGrvRt279HrxgDLGUvpbeWrCsp3D7HdR2QZn-9MZp7CHPlMGtFgN9aIB4Guf7qYlRiC3Ja0ZI22jSMct6xrI-90XX4AK2FhiWmYOmjV_2d3vEVw", "e": "AQAB"}]})).unwrap();
        let kid = keys.keys[0].common.key_id.clone();
        assert!(find_key(&keys, kid.as_deref(), Algorithm::RS256).is_some());
        assert!(find_key(&keys, None, Algorithm::RS256).is_some());
        assert!(find_key(&keys, Some("other"), Algorithm::RS256).is_none());
        assert!(find_key(&keys, kid.as_deref(), Algorithm::ES256).is_none());
    }

    #[tokio::test]
    async fn accepts_a_valid_assertion_once() {
        let verifier = verifier();
        let token = sign(&claims("one"));
        assert!(check(&verifier, &token).await.is_ok());
        assert!(check(&verifier, &token).await.is_err(), "replay must fail");
        assert!(check(&verifier, &sign(&claims("two"))).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_wrong_claims() {
        let verifier = verifier();
        let now = jsonwebtoken::get_current_timestamp();
        let mut cases = Vec::new();
        for (field, value) in [
            ("iss", json!("https://other.example/client.json")),
            ("sub", json!("https://other.example/client.json")),
            ("aud", json!("https://elsewhere.example/oauth/token")),
            ("exp", json!(now - 3600)),
            ("exp", json!(now + 3600)),
        ] {
            let mut claims = claims(&format!("{field}-{value}"));
            claims[field] = value;
            cases.push((field, claims));
        }
        let mut without_jti = claims("unused");
        without_jti.as_object_mut().unwrap().remove("jti");
        cases.push(("jti", without_jti));

        for (field, claims) in cases {
            assert!(
                check(&verifier, &sign(&claims)).await.is_err(),
                "bad {field} accepted"
            );
        }
    }

    #[tokio::test]
    async fn rejects_tampered_and_unsigned_tokens() {
        let verifier = verifier();
        let token = sign(&claims("tampered"));
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let other = sign(&claims("other"));
        let (_, other_signature) = other.rsplit_once('.').unwrap();
        assert!(
            check(&verifier, &format!("{signed}.{other_signature}"))
                .await
                .is_err()
        );
        assert!(check(&verifier, &format!("{signed}.")).await.is_err());
        assert!(!signature.is_empty());
    }
}
