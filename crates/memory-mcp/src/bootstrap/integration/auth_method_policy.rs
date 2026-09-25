use std::sync::Arc;

use crate::MemoryError;
use crate::http::config::{BrowserAuthMethod, HttpConfig};
use crate::http::registry::storage::RegistryStore;
use crate::identity::api::{AuthMethod, AuthMethodPolicy, AuthMethodPolicyPort};

pub(crate) struct RegistryAuthMethodPolicy {
    store: Arc<dyn RegistryStore>,
    config: Option<HttpConfig>,
}

impl RegistryAuthMethodPolicy {
    pub(crate) fn new(store: Arc<dyn RegistryStore>, config: HttpConfig) -> Self {
        Self {
            store,
            config: Some(config),
        }
    }

    pub(crate) fn for_explicit_removal(store: Arc<dyn RegistryStore>) -> Self {
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
