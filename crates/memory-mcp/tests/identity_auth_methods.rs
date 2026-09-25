#![cfg(feature = "control-plane")]

use memory_mcp::MemoryError;
use memory_mcp::identity::api::{
    AuthMethod, AuthMethodPolicy, AuthMethodPolicyPort, IdentityError, reconcile_auth_methods,
    remove_auth_method,
};

#[derive(Default)]
struct RecordingPolicy {
    existing: Vec<AuthMethod>,
}

#[async_trait::async_trait]
impl AuthMethodPolicyPort for RecordingPolicy {
    async fn reconcile(&self, desired: &[AuthMethod]) -> Result<AuthMethodPolicy, MemoryError> {
        let methods = self
            .existing
            .iter()
            .copied()
            .chain(desired.iter().copied())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(AuthMethodPolicy { methods, epoch: 1 })
    }

    async fn remove(&self, method: AuthMethod) -> Result<AuthMethodPolicy, MemoryError> {
        if !self.existing.contains(&method) {
            return Err(MemoryError::Conflict(
                "browser authentication method 'oidc' is not enabled by the durable policy".into(),
            ));
        }
        Ok(AuthMethodPolicy {
            methods: self
                .existing
                .iter()
                .copied()
                .filter(|existing| *existing != method)
                .collect(),
            epoch: 2,
        })
    }
}

#[tokio::test]
async fn method_reconciliation_never_narrows_and_explicit_removal_requires_method() {
    let port = RecordingPolicy {
        existing: vec![AuthMethod::Local],
    };
    reconcile_auth_methods(&port, &[AuthMethod::Local, AuthMethod::Oidc])
        .await
        .expect("additive reconciliation");
    assert!(matches!(
        remove_auth_method(&port, AuthMethod::Oidc).await,
        Err(IdentityError::MethodNotEnabled(_))
    ));
    assert!(remove_auth_method(&port, AuthMethod::Local).await.is_ok());
}
