//! JWKS cache used by [`crate::control::oidc::OidcClient`].
//!
//! The cache is shared via `Arc` so concurrent ID-token validations
//! hit the in-memory `HashMap`; a refresh is serialized through a
//! `tokio::sync::Mutex` and only re-fetches when the cached keys
//! are older than the configured TTL.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use jsonwebtoken::DecodingKey;
use tokio::sync::Mutex;

use super::flow_material::AuthError;

#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<RwLock<JwksState>>,
    refresh_lock: Arc<Mutex<()>>,
    client: reqwest::Client,
    jwks_uri: String,
    ttl: Duration,
}

struct JwksState {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
}

impl JwksState {
    fn empty() -> Self {
        Self {
            keys: HashMap::new(),
            fetched_at: None,
        }
    }
}

impl JwksCache {
    /// Construct a cache from its parts. Used by `OidcClient::new`
    /// after OIDC discovery.
    pub fn from_parts(client: reqwest::Client, jwks_uri: String, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(JwksState::empty())),
            refresh_lock: Arc::new(Mutex::new(())),
            client,
            jwks_uri,
            ttl,
        }
    }
    pub fn find_key(&self, kid: &str) -> Result<Option<DecodingKey>, AuthError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .keys
            .get(kid)
            .cloned())
    }

    pub async fn key_for(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        let fresh = self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh && let Some(key) = self.find_key(kid)? {
            return Ok(key);
        }
        let _refresh = self.refresh_lock.lock().await;
        let fresh = self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh && let Some(key) = self.find_key(kid)? {
            return Ok(key);
        }
        self.refresh().await?;
        self.find_key(kid)?
            .ok_or_else(|| AuthError::Jwks("unknown key id".into()))
    }

    pub async fn refresh(&self) -> Result<(), AuthError> {
        #[derive(serde::Deserialize)]
        struct JwksDocument {
            keys: Vec<Jwk>,
        }
        #[derive(serde::Deserialize)]
        struct Jwk {
            kid: String,
            kty: String,
            n: Option<String>,
            e: Option<String>,
            crv: Option<String>,
            x: Option<String>,
            y: Option<String>,
            alg: Option<String>,
        }
        let document = self
            .client
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|error| AuthError::Jwks(error.to_string()))?
            .error_for_status()
            .map_err(|error| AuthError::Jwks(error.to_string()))?
            .json::<JwksDocument>()
            .await
            .map_err(|error| AuthError::Jwks(error.to_string()))?;
        let mut keys = HashMap::new();
        for jwk in document.keys.into_iter().take(32) {
            let key = match (
                jwk.kty.as_str(),
                jwk.n.as_deref(),
                jwk.e.as_deref(),
                jwk.crv.as_deref(),
                jwk.x.as_deref(),
                jwk.y.as_deref(),
            ) {
                ("RSA", Some(n), Some(e), _, _, _) => {
                    jsonwebtoken::DecodingKey::from_rsa_components(n, e)
                }
                ("EC", _, _, Some("P-256"), Some(x), Some(y)) => {
                    jsonwebtoken::DecodingKey::from_ec_components(x, y)
                }
                ("OKP", _, _, Some("Ed25519"), Some(x), _) => {
                    jsonwebtoken::DecodingKey::from_ed_components(x)
                }
                _ => continue,
            }
            .map_err(|error| AuthError::Jwks(error.to_string()))?;
            if jwk
                .alg
                .as_deref()
                .is_none_or(|alg| alg == "RS256" || alg == "ES256" || alg == "EdDSA")
            {
                keys.insert(jwk.kid, key);
            }
        }
        let mut state = self
            .inner
            .write()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?;
        state.keys = keys;
        state.fetched_at = Some(Instant::now());
        Ok(())
    }
}
