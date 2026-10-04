#![cfg(feature = "control-plane")]

use std::sync::Mutex;

use memory_mcp::MemoryError;
use memory_mcp::identity::api::{
    AuthMethod, AuthMethodPolicy, AuthMethodPolicyPort, IdentityError, reconcile_auth_methods,
    remove_auth_method,
};

#[derive(Default)]
struct RecordingPolicy {
    desired: Mutex<Vec<AuthMethod>>,
    removals: Mutex<Vec<AuthMethod>>,
    removal: Mutex<Option<Result<AuthMethodPolicy, MemoryError>>>,
}

#[async_trait::async_trait]
impl AuthMethodPolicyPort for RecordingPolicy {
    async fn reconcile(&self, desired: &[AuthMethod]) -> Result<AuthMethodPolicy, MemoryError> {
        self.desired
            .lock()
            .expect("desired lock")
            .extend_from_slice(desired);
        Ok(AuthMethodPolicy {
            methods: vec![AuthMethod::Local, AuthMethod::Oidc],
            epoch: 7,
        })
    }

    async fn remove(&self, method: AuthMethod) -> Result<AuthMethodPolicy, MemoryError> {
        self.removals.lock().expect("removals lock").push(method);
        self.removal
            .lock()
            .expect("removal lock")
            .take()
            .expect("configured removal answer")
    }
}

#[tokio::test]
async fn method_reconciliation_forwards_the_requested_policy() {
    let port = RecordingPolicy::default();

    let observed = reconcile_auth_methods(&port, &[AuthMethod::Local, AuthMethod::Oidc])
        .await
        .expect("additive reconciliation");
    assert_eq!(
        port.desired.lock().expect("desired lock").as_slice(),
        [AuthMethod::Local, AuthMethod::Oidc]
    );
    assert_eq!(
        observed,
        AuthMethodPolicy {
            methods: vec![AuthMethod::Local, AuthMethod::Oidc],
            epoch: 7,
        }
    );
}

#[tokio::test]
async fn removing_a_disabled_method_maps_the_store_conflict() {
    let port = RecordingPolicy::default();
    *port.removal.lock().expect("removal lock") = Some(Err(MemoryError::Conflict(
        "browser authentication method 'oidc' is not enabled by the durable policy".into(),
    )));

    assert!(matches!(
        remove_auth_method(&port, AuthMethod::Oidc).await,
        Err(IdentityError::MethodNotEnabled(_))
    ));
    assert_eq!(
        port.removals.lock().expect("removals lock").as_slice(),
        [AuthMethod::Oidc]
    );
}

#[tokio::test]
async fn removing_an_enabled_method_returns_the_store_policy() {
    let port = RecordingPolicy::default();
    let policy = AuthMethodPolicy {
        methods: vec![AuthMethod::Local],
        epoch: 8,
    };
    *port.removal.lock().expect("removal lock") = Some(Ok(policy.clone()));

    assert_eq!(
        remove_auth_method(&port, AuthMethod::Oidc)
            .await
            .expect("removed"),
        policy
    );
    assert_eq!(
        port.removals.lock().expect("removals lock").as_slice(),
        [AuthMethod::Oidc]
    );
}
