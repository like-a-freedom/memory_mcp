#![cfg(feature = "control-plane")]

use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use memory_mcp::MemoryError;
use memory_mcp::identity::api::{
    IdentityError, IdentityLinkTransactions, UnlinkIdentityCommand, unlink_identity,
};

type UnlinkCall = (String, String, String, chrono::DateTime<Utc>);

#[derive(Default)]
struct RecordingIdentityStore {
    calls: Mutex<Vec<UnlinkCall>>,
    error: Mutex<Option<MemoryError>>,
}

#[async_trait::async_trait]
impl IdentityLinkTransactions for RecordingIdentityStore {
    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        actor: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.calls.lock().expect("calls lock").push((
            account_id.to_string(),
            identity_id.to_string(),
            actor.to_string(),
            now,
        ));
        if let Some(error) = self.error.lock().expect("error lock").take() {
            return Err(error);
        }
        Ok(())
    }
}

#[tokio::test]
async fn unlink_identity_requires_recent_auth_and_maps_atomic_refusals() {
    let now = Utc::now();
    let store = Arc::new(RecordingIdentityStore::default());
    let command = UnlinkIdentityCommand {
        account_id: "acct_1".into(),
        identity_id: "idn_1".into(),
        actor: "acct_1".into(),
        authenticated_at: now - Duration::minutes(1),
    };

    unlink_identity(store.as_ref(), &command, now)
        .await
        .expect("accepted unlink");
    assert_eq!(
        *store.calls.lock().expect("calls lock"),
        vec![("acct_1".into(), "idn_1".into(), "acct_1".into(), now)]
    );

    let mut stale = command.clone();
    stale.authenticated_at = now - Duration::minutes(11);
    assert!(matches!(
        unlink_identity(store.as_ref(), &stale, now).await,
        Err(IdentityError::ReauthenticationRequired)
    ));

    *store.error.lock().expect("error lock") = Some(MemoryError::Conflict("last identity".into()));
    assert!(matches!(
        unlink_identity(store.as_ref(), &command, now).await,
        Err(IdentityError::LastIdentityOrConflict)
    ));

    *store.error.lock().expect("error lock") = Some(MemoryError::NotFound("identity".into()));
    assert!(matches!(
        unlink_identity(store.as_ref(), &command, now).await,
        Err(IdentityError::NotFound)
    ));
    assert_eq!(store.calls.lock().expect("calls lock").len(), 3);
}
