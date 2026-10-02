//! Principal resolution.
//!
//! Provides the request-scoped `AuthenticatedPrincipal` and the
//! parser/verifier pieces needed by the auth pipeline: the cache,
//! the account→tenant resolver, and the auth middleware that turns
//! a header into a principal.

pub mod api_keys;
pub mod auth;
pub mod cache;

use std::sync::Arc;

use crate::http::registry::models::Account;

/// The request-scoped authenticated identity. Every namespace
/// decision derives from this value — never from MCP arguments,
/// URL paths, or claims.
#[derive(Clone, Debug)]
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

    pub fn credential_kind(&self) -> &'static str {
        match self {
            Self::ApiKey { .. } => "api_key",
            #[cfg(feature = "control-plane")]
            Self::Oidc { .. } => "oidc",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::{Account, AccountStatus};

    /// An account with a fixed id, so principal projections are comparable.
    fn account(id: &str) -> Arc<Account> {
        Arc::new(Account {
            id: id.to_string(),
            status: AccountStatus::Active,
            tenant_id: "ten-1".to_string(),
            created_at: chrono::Utc::now(),
            display_name: None,
        })
    }

    /// An API-key principal over `id`.
    fn api_key_principal(id: &str) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal::ApiKey {
            account: account(id),
            key_id: "key-1".to_string(),
        }
    }

    #[test]
    fn an_api_key_principal_reports_its_account_id() {
        let principal = api_key_principal("acc-1");

        assert_eq!(principal.account_id(), "acc-1");
    }

    #[test]
    fn an_api_key_principal_reports_the_api_key_credential_kind() {
        let observed = api_key_principal("acc-1").credential_kind();

        assert_eq!(observed, "api_key");
    }

    #[test]
    fn an_api_key_principal_exposes_its_account() {
        let observed = api_key_principal("acc-1").account().id.clone();

        assert_eq!(observed, "acc-1");
    }

    #[test]
    fn a_cloned_principal_keeps_the_same_account_id() {
        let cloned = api_key_principal("acc-1").clone();

        assert_eq!(cloned.account_id(), "acc-1");
    }

    #[cfg(feature = "control-plane")]
    #[test]
    fn an_oidc_principal_reports_its_account_id() {
        let principal = AuthenticatedPrincipal::Oidc {
            account: account("acc-2"),
            issuer: "https://issuer.example".to_string(),
            subject: "sub-1".to_string(),
        };

        assert_eq!(principal.account_id(), "acc-2");
    }

    #[cfg(feature = "control-plane")]
    #[test]
    fn an_oidc_principal_reports_the_oidc_credential_kind() {
        let principal = AuthenticatedPrincipal::Oidc {
            account: account("acc-2"),
            issuer: "https://issuer.example".to_string(),
            subject: "sub-1".to_string(),
        };

        assert_eq!(principal.credential_kind(), "oidc");
    }

    #[cfg(feature = "control-plane")]
    #[test]
    fn the_two_credential_kinds_are_distinguishable() {
        let api_key = api_key_principal("acc-1").credential_kind();
        let oidc = AuthenticatedPrincipal::Oidc {
            account: account("acc-2"),
            issuer: "https://issuer.example".to_string(),
            subject: "sub-1".to_string(),
        }
        .credential_kind();

        assert_ne!(api_key, oidc, "log context must tell the two apart");
    }

    #[cfg(feature = "control-plane")]
    #[test]
    fn an_oidc_principal_exposes_its_account() {
        let principal = AuthenticatedPrincipal::Oidc {
            account: account("acc-2"),
            issuer: "https://issuer.example".to_string(),
            subject: "sub-1".to_string(),
        };

        assert_eq!(principal.account().id.clone(), "acc-2");
    }
}
