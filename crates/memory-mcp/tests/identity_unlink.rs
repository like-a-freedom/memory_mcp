#![cfg(feature = "control-plane")]

use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
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

fn at(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("fixed timestamp")
        .with_timezone(&Utc)
}

fn command(authenticated_at: DateTime<Utc>) -> UnlinkIdentityCommand {
    UnlinkIdentityCommand {
        account_id: "acct_1".into(),
        identity_id: "idn_1".into(),
        actor: "acct_1".into(),
        authenticated_at,
    }
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
async fn unlink_identity_forwards_the_authorized_transaction() {
    let now = at("2026-10-03T12:00:00Z");
    let store = RecordingIdentityStore::default();
    let command = command(now - Duration::minutes(1));

    unlink_identity(&store, &command, now)
        .await
        .expect("accepted unlink");
    assert_eq!(
        store.calls.lock().expect("calls lock").as_slice(),
        vec![("acct_1".into(), "idn_1".into(), "acct_1".into(), now)]
    );
}

#[tokio::test]
async fn stale_authentication_refuses_unlink_before_the_transaction() {
    let now = at("2026-10-03T12:00:00Z");
    let store = RecordingIdentityStore::default();

    assert!(matches!(
        unlink_identity(&store, &command(now - Duration::minutes(11)), now).await,
        Err(IdentityError::ReauthenticationRequired)
    ));
    assert!(store.calls.lock().expect("calls lock").is_empty());
}

#[tokio::test]
async fn unlink_conflict_maps_to_last_identity_refusal() {
    let now = at("2026-10-03T12:00:00Z");
    let store = RecordingIdentityStore::default();
    *store.error.lock().expect("error lock") = Some(MemoryError::Conflict("last identity".into()));

    assert!(matches!(
        unlink_identity(&store, &command(now - Duration::minutes(1)), now).await,
        Err(IdentityError::LastIdentityOrConflict)
    ));
    assert_eq!(store.calls.lock().expect("calls lock").len(), 1);
}

#[tokio::test]
async fn missing_identity_maps_to_not_found() {
    let now = at("2026-10-03T12:00:00Z");
    let store = RecordingIdentityStore::default();
    *store.error.lock().expect("error lock") = Some(MemoryError::NotFound("identity".into()));

    assert!(matches!(
        unlink_identity(&store, &command(now - Duration::minutes(1)), now).await,
        Err(IdentityError::NotFound)
    ));
    assert_eq!(store.calls.lock().expect("calls lock").len(), 1);
}
