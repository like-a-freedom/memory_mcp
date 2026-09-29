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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::storage::InMemoryStore;

    /// A policy adapter over a fresh in-memory store, with no `HttpConfig`
    /// attached (the explicit-removal shape).
    fn adapter() -> RegistryAuthMethodPolicy {
        RegistryAuthMethodPolicy::for_explicit_removal(Arc::new(InMemoryStore::default()))
    }

    #[tokio::test]
    async fn reconciling_no_methods_creates_the_singleton() {
        let adapter = adapter();

        let observed = adapter.reconcile(&[]).await.expect("reconcile succeeds");

        assert!(observed.methods.is_empty());
    }

    #[tokio::test]
    async fn reconciling_oidc_enables_oidc() {
        let adapter = adapter();

        let observed = adapter
            .reconcile(&[AuthMethod::Oidc])
            .await
            .expect("reconcile succeeds");

        assert_eq!(observed.methods, vec![AuthMethod::Oidc]);
    }

    #[tokio::test]
    async fn reconciling_both_methods_enables_both() {
        let adapter = adapter();

        let observed = adapter
            .reconcile(&[AuthMethod::Local, AuthMethod::Oidc])
            .await
            .expect("reconcile succeeds");

        assert_eq!(observed.methods.len(), 2);
    }

    #[tokio::test]
    async fn reconciliation_is_the_union_so_narrowing_is_refused() {
        // Startup reconciliation never drops a method; only the explicit
        // removal path may, and that is what the epoch guards.
        let adapter = adapter();
        adapter
            .reconcile(&[AuthMethod::Local, AuthMethod::Oidc])
            .await
            .expect("first reconcile succeeds");

        let observed = adapter.reconcile(&[AuthMethod::Oidc]).await;

        assert!(
            observed.is_err(),
            "a row holding a method `desired` omits must be refused, not narrowed"
        );
    }

    #[tokio::test]
    async fn reconciling_the_same_set_twice_is_idempotent() {
        let adapter = adapter();
        adapter
            .reconcile(&[AuthMethod::Oidc])
            .await
            .expect("first reconcile succeeds");

        let observed = adapter
            .reconcile(&[AuthMethod::Oidc])
            .await
            .expect("a repeat reconcile is not a narrowing");

        assert_eq!(observed.methods, vec![AuthMethod::Oidc]);
    }

    #[tokio::test]
    async fn a_stored_policy_carries_an_epoch() {
        let adapter = adapter();

        let observed = adapter
            .reconcile(&[AuthMethod::Oidc])
            .await
            .expect("reconcile succeeds");

        assert!(observed.epoch > 0, "a durable policy row carries an epoch");
    }

    #[tokio::test]
    async fn removing_the_only_method_is_refused_as_a_lockout() {
        // The deployment must never be left with no browser door, so the last
        // method cannot be removed.
        let adapter = adapter();
        adapter
            .reconcile(&[AuthMethod::Oidc])
            .await
            .expect("reconcile succeeds");

        let observed = adapter.remove(AuthMethod::Oidc).await;

        assert!(
            matches!(observed, Err(MemoryError::Conflict(_))),
            "removing the last method would lock the deployment out"
        );
    }

    #[tokio::test]
    async fn removing_a_method_that_was_never_enabled_is_refused() {
        // The store treats any removal it cannot perform as a conflict rather
        // than silently succeeding, so an operator never gets a false "done".
        let adapter = adapter();
        adapter
            .reconcile(&[AuthMethod::Oidc])
            .await
            .expect("reconcile succeeds");

        let observed = adapter.remove(AuthMethod::Local).await;

        assert!(matches!(observed, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    async fn removing_local_leaves_the_oidc_method_in_place() {
        let adapter = adapter();
        adapter
            .reconcile(&[AuthMethod::Local, AuthMethod::Oidc])
            .await
            .expect("reconcile succeeds");

        let observed = adapter
            .remove(AuthMethod::Local)
            .await
            .expect("removal succeeds");

        assert_eq!(observed.methods, vec![AuthMethod::Oidc]);
    }

    #[tokio::test]
    async fn the_snapshot_maps_browser_methods_back_to_auth_methods() {
        let observed = policy_snapshot(vec![BrowserAuthMethod::Local, BrowserAuthMethod::Oidc], 4);

        assert_eq!(
            observed,
            AuthMethodPolicy {
                methods: vec![AuthMethod::Local, AuthMethod::Oidc],
                epoch: 4,
            }
        );
    }
}
