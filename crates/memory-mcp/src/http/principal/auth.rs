//! Bearer-token authenticator.
//!
//! The cache and rate limiter live alongside the authenticator in
//! this module so a single file owns the request-path auth
//! behavior.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use lru::LruCache;

use super::AuthenticatedPrincipal;
use super::api_keys::ApiKeyCredential;
use super::cache::PrincipalCache;
use crate::http::registry::RegistryStore;
use crate::http::registry::models::{AccountStatus, ApiKey, ApiKeyStatus};
use crate::http::sync::recover_lock;

#[derive(Debug)]
pub enum AuthDecision {
    Allow(AuthenticatedPrincipal),
    Deny,
    NotApplicable,
}

/// Fixed-window per-`key_id` rate limiter, bounded to `capacity`
/// tracked keys. Evicted keys simply start a fresh window.
pub struct RateLimiter {
    window: Duration,
    max_per_window: u32,
    windows: Mutex<LruCache<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(capacity: usize, window: Duration, max_per_window: u32) -> Self {
        let cap = std::num::NonZeroUsize::new(capacity).expect("capacity is a non-zero constant");
        Self {
            window,
            max_per_window,
            windows: Mutex::new(LruCache::new(cap)),
        }
    }

    pub fn allow(&self, key_id: &str) -> bool {
        let mut g = recover_lock(&self.windows);
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

/// Predicate: is the given `ApiKey` still valid for use? Used by
/// both the request-path verifier and the subscription revalidator
/// (`is_current`).
fn is_key_current(key: &ApiKey, now: DateTime<Utc>) -> bool {
    key.status == ApiKeyStatus::Active && key.expires_at.map(|expiry| expiry > now).unwrap_or(true)
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
        let now = Utc::now();
        let mut verified = false;
        let mut principal: Option<AuthenticatedPrincipal> = None;

        if let Some(cached) = self.cache.get_positive(cred.key_id()) {
            // Preserve the ≤60s revocation bound without weakening
            // secret verification: a cache hit still verifies the
            // supplied secret.
            if cached.verifier.verify(&self.pepper, cred.secret()) {
                verified = true;
                principal = Some(AuthenticatedPrincipal::ApiKey {
                    account: cached.account.clone(),
                    key_id: cred.key_id().to_owned(),
                });
            }
        } else {
            // Store lookup. The verifier check happens against
            // the registry-stored verifier; we re-fetch the
            // verifier field from the store (not from the
            // credential) so a rotated key still works.
            let key = self.store.find_api_key(cred.key_id()).await.ok().flatten();
            if let Some(k) = key
                && is_key_current(&k, now)
                && k.verifier.verify(&self.pepper, cred.secret())
            {
                let account = self
                    .store
                    .find_account_by_id(&k.account_id)
                    .await
                    .ok()
                    .flatten();
                if let Some(account) = account
                    && account.status == AccountStatus::Active
                {
                    let account = Arc::new(account);
                    self.cache.put_positive(
                        cred.key_id().to_string(),
                        account.clone(),
                        k.verifier.clone(),
                    );
                    principal = Some(AuthenticatedPrincipal::ApiKey {
                        account,
                        key_id: cred.key_id().to_owned(),
                    });
                    verified = true;
                }
            }
        }

        if !verified {
            self.cache.put_negative(cred.key_id().to_string());
            return AuthDecision::Deny;
        }

        // Update last_used_at with a monotonic/CAS registry write.
        // A transient telemetry timestamp failure must not turn an
        // already valid request into an authentication failure, and
        // the raw secret is never written.
        let _ = self.store.touch_api_key(cred.key_id(), now).await;
        principal
            .map(AuthDecision::Allow)
            .unwrap_or(AuthDecision::Deny)
    }

    pub async fn is_current(&self, principal: &AuthenticatedPrincipal) -> bool {
        let now = Utc::now();
        match principal {
            AuthenticatedPrincipal::ApiKey { account, key_id } => {
                let key = self.store.find_api_key(key_id).await.ok().flatten();
                let current = self
                    .store
                    .find_account_by_id(&account.id)
                    .await
                    .ok()
                    .flatten();
                let key_ok =
                    key.is_some_and(|k| k.account_id == account.id && is_key_current(&k, now));
                let account_ok = current.is_some_and(|a| a.status == AccountStatus::Active);
                key_ok && account_ok
            }
            #[cfg(feature = "control-plane")]
            AuthenticatedPrincipal::Oidc { account, .. } => self
                .store
                .find_account_by_id(&account.id)
                .await
                .ok()
                .flatten()
                .is_some_and(|a| a.status == AccountStatus::Active),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::{
        Account, AccountStatus, ApiKey, ApiKeyStatus, KeyedVerifier,
    };
    use crate::http::registry::storage::InMemoryStore;

    fn active_account(id: &str, tenant_id: &str) -> Account {
        Account {
            id: id.to_string(),
            status: AccountStatus::Active,
            tenant_id: tenant_id.to_string(),
            created_at: Utc::now(),
        }
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
        let store: Arc<dyn RegistryStore> = Arc::new(InMemoryStore::default());
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
    async fn valid_bearer_returns_allow() {
        // Build a credential whose key_id matches a key the store
        // knows about, and whose secret verifies against the
        // stored verifier.
        let store = Arc::new(InMemoryStore::default());
        let pepper = b"pepper";
        let secret = b"Ab3defghij0123456789Ab3defghij0123456789";
        let k = ApiKey {
            id: "ak_01234567-89ab-4cde-8f01-23456789abcd".into(),
            account_id: "acct_1".into(),
            name: "k1".into(),
            verifier: KeyedVerifier::compute(pepper, secret),
            status: ApiKeyStatus::Active,
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            version: 1,
        };
        store.write_api_key(&k).await.unwrap();
        store
            .write_account(&active_account("acct_1", "ten_1"))
            .await
            .unwrap();
        let auth = Authenticator::new(
            store,
            Arc::new(PrincipalCache::new(8)),
            pepper.to_vec(),
            Arc::new(RateLimiter::new(4, Duration::from_secs(60), 100)),
        );
        let raw = format!(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_{}",
            std::str::from_utf8(secret).unwrap()
        );
        let d = auth.authenticate_bearer(&raw).await;
        match d {
            AuthDecision::Allow(AuthenticatedPrincipal::ApiKey { account, .. }) => {
                assert_eq!(account.id, "acct_1");
            }
            other => panic!("expected Allow(ApiKey), got {other:?}"),
            #[allow(unreachable_patterns)]
            _ => panic!("unreachable"),
        }
    }

    #[tokio::test]
    async fn wrong_secret_returns_deny() {
        let store = Arc::new(InMemoryStore::default());
        let pepper = b"pepper";
        let real_secret = b"Ab3defghij0123456789Ab3defghij0123456789";
        let wrong_secret = b"Bb3defghij0123456789Ab3defghij0123456789";
        let k = ApiKey {
            id: "ak_01234567-89ab-4cde-8f01-23456789abcd".into(),
            account_id: "acct_1".into(),
            name: "k1".into(),
            verifier: KeyedVerifier::compute(pepper, real_secret),
            status: ApiKeyStatus::Active,
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            version: 1,
        };
        store.write_api_key(&k).await.unwrap();
        store
            .write_account(&active_account("acct_1", "ten_1"))
            .await
            .unwrap();
        let auth = Authenticator::new(
            store,
            Arc::new(PrincipalCache::new(8)),
            pepper.to_vec(),
            Arc::new(RateLimiter::new(4, Duration::from_secs(60), 100)),
        );
        let raw = format!(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_{}",
            std::str::from_utf8(wrong_secret).unwrap()
        );
        let d = auth.authenticate_bearer(&raw).await;
        assert!(matches!(d, AuthDecision::Deny));
    }
}
