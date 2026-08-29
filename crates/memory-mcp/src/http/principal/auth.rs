//! Bearer-token authenticator (ADR-0052, plan §4.4).
//!
//! The cache and rate limiter live alongside the authenticator in
//! this module so a single file owns the request-path auth
//! behavior. The OIDC branch lands in Task 4.6.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;

use super::api_keys::ApiKeyCredential;
use super::cache::PrincipalCache;
use super::AuthenticatedPrincipal;
use crate::http::registry::models::{AccountStatus, ApiKeyStatus};
use crate::http::registry::RegistryStore;

pub enum AuthDecision {
    Allow(AuthenticatedPrincipal),
    Deny,
    NotApplicable,
}

/// Fixed-window per-`key_id` rate limiter, bounded to `capacity`
/// tracked keys (spec §12). Evicted keys simply start a fresh
/// window.
pub struct RateLimiter {
    window: Duration,
    max_per_window: u32,
    windows: Mutex<LruCache<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(capacity: usize, window: Duration, max_per_window: u32) -> Self {
        let cap = std::num::NonZeroUsize::new(capacity)
            .expect("capacity is a non-zero constant");
        Self {
            window,
            max_per_window,
            windows: Mutex::new(LruCache::new(cap)),
        }
    }

    pub fn allow(&self, key_id: &str) -> bool {
        let mut g = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        match g.get_mut(key_id) {
            Some((start, count)) if now.duration_since(*start) < self.window => {
                if *count >= self.max_per_window {
                    return false;
                }
                *count += 1;
                true
            }
            _ => {
                g.put(key_id.to_string(), (now, 1));
                true
            }
        }
    }
}

pub struct Authenticator {
    store: Arc<dyn RegistryStore>,
    cache: Arc<PrincipalCache>,
    pepper: Arc<Vec<u8>>,
    rate_limiter: Arc<RateLimiter>,
}

impl Authenticator {
    pub fn new(
        store: Arc<dyn RegistryStore>,
        cache: Arc<PrincipalCache>,
        pepper: Vec<u8>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            store,
            cache,
            pepper: Arc::new(pepper),
            rate_limiter,
        }
    }

    pub async fn authenticate_bearer(&self, header: &str) -> AuthDecision {
        let cred = match ApiKeyCredential::parse(header) {
            Ok(c) => c,
            Err(_) => return AuthDecision::Deny,
        };
        if self.cache.get_negative(cred.key_id()) {
            return AuthDecision::Deny;
        }
        if !self.rate_limiter.allow(cred.key_id()) {
            return AuthDecision::Deny;
        }
        if let Some(cached) = self.cache.get_positive(cred.key_id()) {
            // Preserve the ≤60s revocation bound without weakening
            // secret verification: a cache hit still verifies the
            // supplied secret.
            if cached.verifier.verify(&self.pepper, cred.secret()) {
                let _ = self
                    .store
                    .touch_api_key(cred.key_id(), chrono::Utc::now())
                    .await;
                return AuthDecision::Allow(AuthenticatedPrincipal::ApiKey {
                    account: cached.account.clone(),
                    key_id: cred.key_id().to_owned(),
                });
            }
            self.cache.put_negative(cred.key_id().to_string());
            return AuthDecision::Deny;
        }
        let key = match self.store.find_api_key(cred.key_id()).await {
            Ok(Some(k))
                if k.status == ApiKeyStatus::Active
                    && k.expires_at
                        .map(|e| e > chrono::Utc::now())
                        .unwrap_or(true)
                    && k.verifier.verify(&self.pepper, cred.secret()) =>
            {
                k
            }
            _ => {
                self.cache.put_negative(cred.key_id().to_string());
                return AuthDecision::Deny;
            }
        };
        let account = match self.store.find_account_by_id(&key.account_id).await {
            Ok(Some(a)) if a.status == AccountStatus::Active => Arc::new(a),
            _ => {
                self.cache.put_negative(cred.key_id().to_string());
                return AuthDecision::Deny;
            }
        };
        self.cache.put_positive(
            cred.key_id().to_string(),
            account.clone(),
            key.verifier.clone(),
        );
        // Update last_used_at with a monotonic/CAS registry write.
        // A transient telemetry timestamp failure must not turn an
        // already valid request into an authentication failure, and
        // the raw secret is never written.
        let _ = self.store.touch_api_key(&key.id, chrono::Utc::now()).await;
        AuthDecision::Allow(AuthenticatedPrincipal::ApiKey {
            account,
            key_id: cred.key_id().to_owned(),
        })
    }

    pub async fn is_current(&self, principal: &AuthenticatedPrincipal) -> bool {
        match principal {
            AuthenticatedPrincipal::ApiKey { account, key_id } => {
                matches!(
                    self.store.find_api_key(key_id).await,
                    Ok(Some(key)) if key.account_id == account.id
                        && key.status == ApiKeyStatus::Active
                        && key
                            .expires_at
                            .map(|expiry| expiry > chrono::Utc::now())
                            .unwrap_or(true)
                ) && matches!(
                    self.store.find_account_by_id(&account.id).await,
                    Ok(Some(current)) if current.status == AccountStatus::Active
                )
            }
            #[cfg(feature = "control-plane")]
            AuthenticatedPrincipal::Oidc { account, .. } => matches!(
                self.store.find_account_by_id(&account.id).await,
                Ok(Some(current)) if current.status == AccountStatus::Active
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::{ApiKey, KeyedVerifier};
    use crate::http::registry::storage::SurrealRegistryStore;
    use crate::http::registry::RegistryHandle;

    fn store_with_key(secret: &[u8], pepper: &[u8]) -> (Arc<dyn RegistryStore>, KeyedVerifier) {
        let verifier = KeyedVerifier::compute(pepper, secret);
        let store: Arc<dyn RegistryStore> = Arc::new(SurrealRegistryStore::new_unconnected());
        // The unconnected store returns Ok(None) for every query,
        // which the authenticator treats as Deny. Useful for the
        // path-only tests below.
        (store, verifier)
    }

    #[tokio::test]
    async fn rate_limiter_caps_then_allows_after_window() {
        let limiter = RateLimiter::new(4, Duration::from_millis(50), 2);
        assert!(limiter.allow("ak_1"));
        assert!(limiter.allow("ak_1"));
        assert!(!limiter.allow("ak_1"));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(limiter.allow("ak_1"));
    }

    #[tokio::test]
    async fn rate_limiter_keys_are_independent() {
        let limiter = RateLimiter::new(4, Duration::from_secs(60), 1);
        assert!(limiter.allow("ak_1"));
        assert!(!limiter.allow("ak_1"));
        assert!(limiter.allow("ak_2"));
    }

    #[tokio::test]
    async fn unparseable_header_returns_deny() {
        let (store, _v) = store_with_key(b"x", b"p");
        let auth = Authenticator::new(
            store,
            Arc::new(PrincipalCache::new(8)),
            b"p".to_vec(),
            Arc::new(RateLimiter::new(4, Duration::from_secs(60), 100)),
        );
        let d = auth.authenticate_bearer("not-a-key").await;
        assert!(matches!(d, AuthDecision::Deny));
    }

    #[tokio::test]
    async fn registry_handle_exposes_store() {
        // Sanity: RegistryHandle is the bridge from HttpState to
        // the Arc<dyn RegistryStore> the authenticator takes.
        let handle = RegistryHandle::stub();
        let _ = handle.ping().await;
    }

    #[allow(dead_code)]
    fn _check_api_key_shape() {
        // Reference-only: a fully-populated ApiKey round-trips
        // through serde. The actual round-trip is exercised in
        // the registry models test module.
        let k = ApiKey {
            id: "ak_test".into(),
            account_id: "acct_test".into(),
            name: "test".into(),
            verifier: KeyedVerifier([1u8; 32]),
            status: ApiKeyStatus::Active,
            created_at: chrono::Utc::now(),
            expires_at: None,
            last_used_at: None,
            version: 1,
        };
        assert_eq!(k.status, ApiKeyStatus::Active);
    }
}
