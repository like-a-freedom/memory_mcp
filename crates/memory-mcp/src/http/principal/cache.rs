//! LRU caches for authentication decisions (ADR-0052, plan §4.4).
//!
//! Two caches live here:
//!
//! - **positive** caches `(Account, KeyedVerifier)` keyed by
//!   `key_id`. Caching the verifier alongside the account matters:
//!   a cache hit must still verify the supplied secret, otherwise a
//!   revoked credential would be accepted until TTL expiry.
//! - **negative** caches recently-rejected `key_id`s so a flood of
//!   forged credentials does not translate into a flood of
//!   registry reads.
//!
//! Both caches use `std::sync::Mutex` with poisoned-guard recovery;
//! the workspace does not depend on `parking_lot`, and adding it
//! for this one hot path would violate the dependency gate.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;

use crate::http::registry::models::{Account, KeyedVerifier};
use crate::http::sync::recover_lock;

const POSITIVE_TTL: Duration = Duration::from_secs(60);
const NEGATIVE_TTL: Duration = Duration::from_secs(5);

pub struct CachedPrincipal {
    pub account: Arc<Account>,
    pub verifier: KeyedVerifier,
}

pub struct PrincipalCache {
    positive: Mutex<LruCache<String, (Arc<CachedPrincipal>, Instant)>>,
    negative: Mutex<LruCache<String, Instant>>,
}

impl PrincipalCache {
    pub fn new(capacity: usize) -> Self {
        let cap = std::num::NonZeroUsize::new(capacity).expect("capacity is a non-zero constant");
        Self {
            positive: Mutex::new(LruCache::new(cap)),
            negative: Mutex::new(LruCache::new(cap)),
        }
    }

    pub fn get_positive(&self, key_id: &str) -> Option<Arc<CachedPrincipal>> {
        let mut g = recover_lock(&self.positive);
        let v = g.get(key_id)?;
        if v.1.elapsed() > POSITIVE_TTL {
            g.pop(key_id);
            None
        } else {
            Some(v.0.clone())
        }
    }

    pub fn put_positive(&self, key_id: String, account: Arc<Account>, verifier: KeyedVerifier) {
        let cached = Arc::new(CachedPrincipal { account, verifier });
        recover_lock(&self.positive).put(key_id, (cached, Instant::now()));
    }

    pub fn get_negative(&self, key_id: &str) -> bool {
        let mut g = recover_lock(&self.negative);
        match g.get(key_id) {
            Some(t) if t.elapsed() > NEGATIVE_TTL => {
                g.pop(key_id);
                false
            }
            Some(_) => true,
            None => false,
        }
    }

    pub fn put_negative(&self, key_id: String) {
        recover_lock(&self.negative).put(key_id, Instant::now());
    }

    pub fn invalidate(&self, key_id: &str) {
        recover_lock(&self.positive).pop(key_id);
        recover_lock(&self.negative).pop(key_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::{Account, AccountStatus, KeyedVerifier};

    fn account(id: &str) -> Account {
        Account {
            id: id.to_string(),
            status: AccountStatus::Active,
            tenant_id: "ten_test".into(),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn negative_cache_swallows_repeated_unknown_keys() {
        let c = PrincipalCache::new(8);
        assert!(!c.get_negative("ak_1"));
        c.put_negative("ak_1".into());
        assert!(c.get_negative("ak_1"));
        assert!(c.get_negative("ak_1"));
    }

    #[test]
    fn positive_cache_returns_arc_within_ttl() {
        let c = PrincipalCache::new(8);
        let a = Arc::new(account("acct_1"));
        c.put_positive("ak_1".into(), a.clone(), KeyedVerifier([0; 32]));
        let got = c.get_positive("ak_1").expect("hit");
        assert_eq!(got.account.id, "acct_1");
    }

    #[test]
    fn invalidate_clears_both_caches() {
        let c = PrincipalCache::new(8);
        c.put_positive(
            "ak_1".into(),
            Arc::new(account("acct_1")),
            KeyedVerifier([0; 32]),
        );
        c.put_negative("ak_1".into());
        c.invalidate("ak_1");
        assert!(c.get_positive("ak_1").is_none());
        assert!(!c.get_negative("ak_1"));
    }
}
