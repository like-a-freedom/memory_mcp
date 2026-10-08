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
use crate::http::principal::cache::PrincipalCache;
use crate::http::registry::models::{AccountStatus, ApiKey, ApiKeyStatus};
use crate::http::registry::storage::{AccountStore, ApiKeyStore};
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
        let cap =
            std::num::NonZeroUsize::new(capacity.max(1)).unwrap_or(std::num::NonZeroUsize::MIN);
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

/// Verifies API-key credentials against the durable stores.
///
/// Two owner traits, and the pair is what verification actually needs: a key
/// is only valid while its account is active, so `is_current` revalidates both.
/// It used to hold the whole registry, which also let it mutate tenants,
/// sessions and browser policy.
pub struct Authenticator {
    accounts: Arc<dyn AccountStore>,
    api_keys: Arc<dyn ApiKeyStore>,
    cache: Arc<PrincipalCache>,
    pepper: Arc<Vec<u8>>,
    rate_limiter: Arc<RateLimiter>,
}

impl Authenticator {
    pub fn new(
        accounts: Arc<dyn AccountStore>,
        api_keys: Arc<dyn ApiKeyStore>,
        cache: Arc<PrincipalCache>,
        pepper: Vec<u8>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            accounts,
            api_keys,
            cache,
            pepper: Arc::new(pepper),
            rate_limiter,
        }
    }

    pub async fn authenticate_bearer(&self, header: &str) -> AuthDecision {
        let cred = match ApiKeyCredential::parse(header) {
            Ok(c) => c,
            Err(_) => {
                crate::http::logging::log_auth_rejection(
                    crate::http::logging::AuthRejection::Parse,
                    None,
                );
                return AuthDecision::Deny;
            }
        };
        if self.cache.get_negative(cred.key_id()) {
            crate::http::logging::log_auth_rejection(
                crate::http::logging::AuthRejection::CachedRejection,
                None,
            );
            return AuthDecision::Deny;
        }
        if !self.rate_limiter.allow(cred.key_id()) {
            crate::http::logging::log_auth_rejection(
                crate::http::logging::AuthRejection::RateLimited,
                None,
            );
            return AuthDecision::Deny;
        }
        let now = Utc::now();
        let mut verified = false;
        let mut principal: Option<AuthenticatedPrincipal> = None;

        if let Some(cached) = self.cache.get_positive(cred.key_id()) {
            // Re-validate the ApiKey status/expiry on every cache hit so
            // a revoked or expired key is rejected within one request
            // round-trip rather than waiting for the 60 s positive TTL.
            let current_key = self
                .api_keys
                .find_api_key(cred.key_id())
                .await
                .ok()
                .flatten();
            // Ownership is re-read alongside status/expiry: a key that no
            // longer belongs to the cached account must not keep acting as
            // that account just because its verifier still matches.
            let key_valid = current_key
                .as_ref()
                .is_some_and(|k| is_key_current(k, now) && k.account_id == cached.account.id);
            // A positive cache hit still re-reads the account status. This
            // closes the deletion revocation window without requiring the
            // cache to become a second source of account lifecycle truth.
            let current_account = self
                .accounts
                .find_account_by_id(&cached.account.id)
                .await
                .ok()
                .flatten();
            if key_valid
                && cached.verifier.verify(&self.pepper, cred.secret())
                && let Some(account) = current_account
                && account.status == AccountStatus::Active
            {
                verified = true;
                principal = Some(AuthenticatedPrincipal::ApiKey {
                    account: Arc::new(account),
                    key_id: cred.key_id().to_owned(),
                });
            } else {
                // Stale entry: key revoked/expired, secret mismatch, or
                // account suspended/deleted. Evict from both caches so the
                // next request performs a fresh store lookup.
                self.cache.invalidate(cred.key_id());
            }
        } else {
            // Store lookup. The verifier check happens against
            // the registry-stored verifier; we re-fetch the
            // verifier field from the store (not from the
            // credential) so a rotated key still works.
            let key = self
                .api_keys
                .find_api_key(cred.key_id())
                .await
                .ok()
                .flatten();
            if let Some(k) = key.as_ref()
                && is_key_current(k, now)
                && k.verifier.verify(&self.pepper, cred.secret())
            {
                let account = self
                    .accounts
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
            } else if key.is_some() {
                // Key exists in the store but is expired, revoked, or the
                // secret does not match. Clear any stale cache entries so
                // a negative cache entry is recorded below.
                self.cache.invalidate(cred.key_id());
            }
        }

        if !verified {
            self.cache.put_negative(cred.key_id().to_string());
            crate::http::logging::log_auth_rejection(
                crate::http::logging::AuthRejection::VerifyFailed,
                None,
            );
            return AuthDecision::Deny;
        }

        // Update last_used_at with a monotonic/CAS registry write.
        // A transient telemetry timestamp failure must not turn an
        // already valid request into an authentication failure, and
        // the raw secret is never written. The failure is still recorded —
        // at `DEBUG`, because this write is pure telemetry — so a wedge in
        // the key store is visible without turning it into a login outage.
        if let Err(error) = self.api_keys.touch_api_key(cred.key_id(), now).await {
            crate::logging::emit(
                std::collections::HashMap::from([
                    (
                        "op".to_string(),
                        serde_json::json!("http.auth.touch_failed"),
                    ),
                    ("error".to_string(), serde_json::json!(error.to_string())),
                ]),
                crate::logging::LogLevel::Debug,
            );
        }
        principal
            .map(AuthDecision::Allow)
            .unwrap_or(AuthDecision::Deny)
    }

    pub async fn is_current(&self, principal: &AuthenticatedPrincipal) -> bool {
        let now = Utc::now();
        match principal {
            AuthenticatedPrincipal::ApiKey { account, key_id } => {
                let key = self.api_keys.find_api_key(key_id).await.ok().flatten();
                let current = self
                    .accounts
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
                .accounts
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
    use crate::error::MemoryError;
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
            display_name: None,
        }
    }

    /// An API-key store that returns one active key and whose telemetry touch
    /// always fails — the discarded failure `authenticate_bearer` must record
    /// without turning a valid request into a refusal.
    struct TouchFails {
        key: ApiKey,
    }

    #[async_trait::async_trait]
    impl ApiKeyStore for TouchFails {
        async fn find_api_key(&self, key_id: &str) -> Result<Option<ApiKey>, MemoryError> {
            Ok((key_id == self.key.id).then(|| self.key.clone()))
        }

        async fn write_api_key(&self, _key: &ApiKey) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn list_api_keys(
            &self,
            _account_id: &str,
        ) -> Result<Vec<crate::http::registry::models::ApiKeyMeta>, MemoryError> {
            Ok(Vec::new())
        }

        async fn revoke_api_key(
            &self,
            _account_id: &str,
            _key_id: &str,
        ) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn touch_api_key(
            &self,
            _key_id: &str,
            _used_at: DateTime<Utc>,
        ) -> Result<(), MemoryError> {
            Err(MemoryError::Storage("key store unavailable".to_string()))
        }

        async fn create_api_key_if_below_limit(
            &self,
            _key: &ApiKey,
            _max_active: u32,
        ) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn revoke_all_api_keys(&self, _account_id: &str) -> Result<u64, MemoryError> {
            Ok(0)
        }
    }

    /// A telemetry write that fails is recorded at `DEBUG` and the request is
    /// still allowed: `touch_api_key` timestamps a key, and a wedge in that
    /// write must not become a login outage. Before this, the failure was
    /// discarded with no trace at all.
    #[tokio::test]
    async fn a_failed_telemetry_touch_is_logged_at_debug_and_still_allows() {
        let pepper = b"pepper";
        let secret = b"Ab3defghij0123456789Ab3defghij0123456789";
        let key = ApiKey {
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
        let stores =
            crate::http::registry::RegistryStores::from_backend(Arc::new(InMemoryStore::default()));
        stores
            .accounts()
            .write_account(&active_account("acct_1", "ten_1"))
            .await
            .unwrap();
        let auth = Authenticator::new(
            stores.accounts(),
            Arc::new(TouchFails { key }),
            Arc::new(PrincipalCache::new(8)),
            pepper.to_vec(),
            Arc::new(RateLimiter::new(4, Duration::from_secs(60), 100)),
        );
        let sink = crate::logging::capture::install();
        let raw = format!(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_{}",
            std::str::from_utf8(secret).unwrap()
        );

        let decision = crate::logging::capture::with_level("debug", || async {
            auth.authenticate_bearer(&raw).await
        })
        .await;

        assert!(
            matches!(decision, AuthDecision::Allow(_)),
            "a telemetry failure must not refuse a valid key"
        );
        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("op=http.auth.touch_failed") && line.contains("DEBUG")),
            "the discarded telemetry failure must be recorded at DEBUG: {recorded:?}"
        );
    }

    /// A rate-limited key is refused at `WARN` with a bounded reason, so an
    /// operator can see refusals as a rate rather than reading the raw header.
    #[tokio::test]
    async fn a_rate_limited_key_is_logged_with_reason_rate_limited() {
        // A well-formed credential: `ApiKeyCredential::parse` must accept it so
        // the refusal reaches the rate limiter rather than the parse branch.
        let credential = "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_Ab3defghij0123456789Ab3defghij0123456789";
        // The limiter allows an unseen key once, so prime it — the second
        // attempt within the window is the one refused.
        let limiter = Arc::new(RateLimiter::new(4, Duration::from_secs(60), 1));
        assert!(limiter.allow("ak_01234567-89ab-4cde-8f01-23456789abcd"));
        let auth = Authenticator::new(
            Arc::new(InMemoryStore::default()),
            Arc::new(InMemoryStore::default()),
            Arc::new(PrincipalCache::new(8)),
            b"pepper".to_vec(),
            limiter,
        );
        let sink = crate::logging::capture::install();

        let decision = auth.authenticate_bearer(credential).await;

        assert!(matches!(decision, AuthDecision::Deny));
        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("op=http.auth.rejected")
                    && line.contains("reason=rate_limited")),
            "the refusal must name its bounded reason: {recorded:?}"
        );
    }

    /// The raw credential is never written, whatever the refusal.
    #[tokio::test]
    async fn a_refusal_line_never_contains_the_credential() {
        const SECRET: &str = "Ab3defghij0123456789Ab3defghij0123456789";
        let credential = format!("mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_{SECRET}");
        let auth = Authenticator::new(
            Arc::new(InMemoryStore::default()),
            Arc::new(InMemoryStore::default()),
            Arc::new(PrincipalCache::new(8)),
            b"pepper".to_vec(),
            Arc::new(RateLimiter::new(4, Duration::from_secs(60), 100)),
        );
        let sink = crate::logging::capture::install();

        let decision = auth.authenticate_bearer(&credential).await;

        assert!(matches!(decision, AuthDecision::Deny));
        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("op=http.auth.rejected")),
            "the refusal must be recorded: {recorded:?}"
        );
        assert!(
            !recorded.iter().any(|line| line.contains(SECRET)),
            "the raw credential must never be logged: {recorded:?}"
        );
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
        let stores =
            crate::http::registry::RegistryStores::from_backend(Arc::new(InMemoryStore::default()));
        let auth = Authenticator::new(
            stores.accounts(),
            stores.api_keys(),
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
        let stores = crate::http::registry::RegistryStores::from_backend(store.clone());
        stores
            .accounts()
            .write_account(&active_account("acct_1", "ten_1"))
            .await
            .unwrap();
        stores.api_keys().write_api_key(&k).await.unwrap();
        let auth = Authenticator::new(
            stores.accounts(),
            stores.api_keys(),
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
        let stores = crate::http::registry::RegistryStores::from_backend(store.clone());
        stores
            .accounts()
            .write_account(&active_account("acct_1", "ten_1"))
            .await
            .unwrap();
        stores.api_keys().write_api_key(&k).await.unwrap();
        let auth = Authenticator::new(
            stores.accounts(),
            stores.api_keys(),
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

    /// A warm cache entry is only a hint: the key row's owner is re-read on
    /// every hit, so an entry that names a different Account than the key
    /// now belongs to cannot authorize that Account.
    #[tokio::test]
    async fn cache_hit_with_a_mismatched_owner_is_denied() {
        let store = Arc::new(InMemoryStore::default());
        let stores = crate::http::registry::RegistryStores::from_backend(store.clone());
        let pepper = b"pepper";
        let secret = b"Ab3defghij0123456789Ab3defghij0123456789";
        let verifier = KeyedVerifier::compute(pepper, secret);
        let key_id = "ak_01234567-89ab-4cde-8f01-23456789abcd";
        stores
            .api_keys()
            .write_api_key(&ApiKey {
                id: key_id.into(),
                account_id: "acct_owner".into(),
                name: "k1".into(),
                verifier: verifier.clone(),
                status: ApiKeyStatus::Active,
                created_at: Utc::now(),
                expires_at: None,
                last_used_at: None,
                version: 1,
            })
            .await
            .unwrap();
        stores
            .accounts()
            .write_account(&active_account("acct_owner", "ten_1"))
            .await
            .unwrap();
        stores
            .accounts()
            .write_account(&active_account("acct_other", "ten_2"))
            .await
            .unwrap();
        let cache = Arc::new(PrincipalCache::new(8));
        let auth = Authenticator::new(
            stores.accounts(),
            stores.api_keys(),
            cache.clone(),
            pepper.to_vec(),
            Arc::new(RateLimiter::new(4, Duration::from_secs(60), 100)),
        );
        let raw = format!("mem_sk_{key_id}_{}", std::str::from_utf8(secret).unwrap());

        // Seed the cache with the wrong Account for this key id, using the
        // real verifier so only the ownership rule can reject the request.
        cache.put_positive(
            key_id.to_string(),
            Arc::new(active_account("acct_other", "ten_2")),
            verifier.clone(),
        );
        assert!(
            matches!(auth.authenticate_bearer(&raw).await, AuthDecision::Deny),
            "a cache entry naming the wrong owner must not authorize"
        );

        // The same warm cache with the correct owner still authorizes.
        cache.invalidate(key_id);
        cache.put_positive(
            key_id.to_string(),
            Arc::new(active_account("acct_owner", "ten_1")),
            verifier,
        );
        match auth.authenticate_bearer(&raw).await {
            AuthDecision::Allow(AuthenticatedPrincipal::ApiKey { account, .. }) => {
                assert_eq!(account.id, "acct_owner");
            }
            other => panic!("expected Allow(ApiKey), got {other:?}"),
        }
    }
}
