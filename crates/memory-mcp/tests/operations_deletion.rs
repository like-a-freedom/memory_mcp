#![cfg(feature = "control-plane")]

use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use memory_mcp::operations::api::{
    AccountDeletionPort, BeginAccountDeletionCommand, OperationsError, begin_account_deletion,
};

type DeletionCall = (String, String, String, chrono::DateTime<Utc>);

#[derive(Default)]
struct RecordingPort {
    calls: Mutex<Vec<DeletionCall>>,
}

#[async_trait::async_trait]
impl AccountDeletionPort for RecordingPort {
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), memory_mcp::MemoryError> {
        self.calls.lock().expect("calls lock").push((
            verifier.to_string(),
            account_id.to_string(),
            session_id.to_string(),
            now,
        ));
        Ok(())
    }
}

fn at(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("fixed timestamp")
        .with_timezone(&Utc)
}

fn command(authenticated_at: DateTime<Utc>, typed_phrase: &str) -> BeginAccountDeletionCommand {
    BeginAccountDeletionCommand {
        account_id: "acct_1".into(),
        session_id: "session_1".into(),
        authenticated_at,
        typed_phrase: typed_phrase.into(),
        challenge_verifier: "verifier_1".into(),
    }
}

#[tokio::test]
async fn confirmed_deletion_forwards_one_atomic_command() {
    let now = at("2026-10-03T12:00:00Z");
    let port = RecordingPort::default();
    let command = command(now - Duration::minutes(1), " DELETE my account ");

    begin_account_deletion(&port, &command, now)
        .await
        .expect("accepted command");

    assert_eq!(
        port.calls.lock().expect("calls lock").as_slice(),
        vec![(
            "verifier_1".into(),
            "acct_1".into(),
            "session_1".into(),
            now,
        )]
    );
}

#[tokio::test]
async fn deletion_refuses_stale_authentication_before_the_atomic_command() {
    let now = at("2026-10-03T12:00:00Z");
    let port = RecordingPort::default();

    assert!(matches!(
        begin_account_deletion(
            &port,
            &command(now - Duration::minutes(11), "DELETE my account"),
            now,
        )
        .await,
        Err(OperationsError::ReauthenticationRequired)
    ));
    assert!(port.calls.lock().expect("calls lock").is_empty());
}

#[tokio::test]
async fn deletion_refuses_a_wrong_confirmation_phrase_before_the_atomic_command() {
    let now = at("2026-10-03T12:00:00Z");
    let port = RecordingPort::default();

    assert!(matches!(
        begin_account_deletion(
            &port,
            &command(now - Duration::minutes(1), "delete my account"),
            now,
        )
        .await,
        Err(OperationsError::ConfirmationPhraseRejected)
    ));
    assert!(port.calls.lock().expect("calls lock").is_empty());
}
