use std::sync::Arc;

use crate::MemoryError;
use crate::http::config::{BrowserAuthMethod, HttpConfig};
use crate::http::registry::storage::BrowserPolicyStore;
use crate::identity::api::{AuthMethod, AuthMethodPolicy, AuthMethodPolicyPort};

/// The durable browser-auth policy, behind the one trait that owns it.
///
/// Both methods this port needs live on `BrowserPolicyStore` and nowhere else,
/// so the adapter takes that trait rather than the registry it used to be
/// handed. The capability it gives up — issuing API keys, mutating tenants —
/// was never its to exercise.
pub(crate) struct RegistryAuthMethodPolicy {
    store: Arc<dyn BrowserPolicyStore>,
    config: Option<HttpConfig>,
}

impl RegistryAuthMethodPolicy {
    pub(crate) fn new(store: Arc<dyn BrowserPolicyStore>, config: HttpConfig) -> Self {
        Self {
            store,
            config: Some(config),
        }
    }

    pub(crate) fn for_explicit_removal(store: Arc<dyn BrowserPolicyStore>) -> Self {
        Self {
            store,
            config: None,
        }
    }
}

#[async_trait::async_trait]
impl AuthMethodPolicyPort for RegistryAuthMethodPolicy {
    async fn reconcile(&self, desired: &[AuthMethod]) -> Result<AuthMethodPolicy, MemoryError> {
        let desired = desired
            .iter()
            .map(|method| match method {
                AuthMethod::Local => BrowserAuthMethod::Local,
                AuthMethod::Oidc => BrowserAuthMethod::Oidc,
            })
            .collect::<Vec<_>>();
        let local = self
            .config
            .as_ref()
            .and_then(|config| config.browser_auth.as_ref())
            .and_then(|methods| methods.local.as_ref())
            .map(|local| {
                crate::service::local_admin::auth::compute_fingerprints(
                    &local.session_key,
                    &local.csrf_key,
                )
            })
            .transpose()
            .map_err(|error| MemoryError::Auth(error.to_string()))?;
        let policy = self.store.reconcile_browser_policy(&desired, local).await?;
        Ok(policy_snapshot(policy.methods, policy.epoch))
    }

    async fn remove(&self, method: AuthMethod) -> Result<AuthMethodPolicy, MemoryError> {
        let method = match method {
            AuthMethod::Local => BrowserAuthMethod::Local,
            AuthMethod::Oidc => BrowserAuthMethod::Oidc,
        };
        let policy = self.store.remove_browser_auth_method(method).await?;
        Ok(policy_snapshot(policy.methods, policy.epoch))
    }
}

fn policy_snapshot(methods: Vec<BrowserAuthMethod>, epoch: u64) -> AuthMethodPolicy {
    AuthMethodPolicy {
        methods: methods
            .into_iter()
            .map(|method| match method {
                BrowserAuthMethod::Local => AuthMethod::Local,
                BrowserAuthMethod::Oidc => AuthMethod::Oidc,
            })
            .collect(),
        epoch,
    }
}
