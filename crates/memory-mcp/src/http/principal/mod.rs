//! Principal resolution (ADR-0052, plan §4.3-4.6).
//!
//! Phase 4 introduces the request-scoped `AuthenticatedPrincipal`
//! and the parser/verifier pieces needed by the auth pipeline.
//! Tasks 4.4-4.7 add the cache, the account→tenant resolver, and
//! the auth middleware that turns a header into a principal.

pub mod api_keys;
pub mod auth;
pub mod cache;

use std::sync::Arc;

use crate::http::registry::models::Account;

/// The request-scoped authenticated identity. Every namespace
/// decision derives from this value — never from MCP arguments,
/// URL paths, or claims.
#[derive(Clone)]
pub enum AuthenticatedPrincipal {
    ApiKey {
        account: Arc<Account>,
        key_id: String,
    },
    #[cfg(feature = "control-plane")]
    Oidc {
        account: Arc<Account>,
        issuer: String,
        /// Verified raw claim retained only in transient request memory.
        subject: String,
    },
}

impl AuthenticatedPrincipal {
    pub fn account_id(&self) -> &str {
        match self {
            Self::ApiKey { account, .. } => &account.id,
            #[cfg(feature = "control-plane")]
            Self::Oidc { account, .. } => &account.id,
        }
    }

    pub fn account(&self) -> &Arc<Account> {
        match self {
            Self::ApiKey { account, .. } => account,
            #[cfg(feature = "control-plane")]
            Self::Oidc { account, .. } => account,
        }
    }
}
