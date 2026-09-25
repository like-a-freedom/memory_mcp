#![cfg(feature = "control-plane")]

use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
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

#[tokio::test]
async fn account_deletion_requires_recent_auth_exact_phrase_and_one_atomic_call() {
    let now = Utc::now();
    let port = Arc::new(RecordingPort::default());
    let command = BeginAccountDeletionCommand {
        account_id: "acct_1".into(),
        session_id: "session_1".into(),
        authenticated_at: now - Duration::minutes(1),
        typed_phrase: " DELETE my account ".into(),
        challenge_verifier: "verifier_1".into(),
    };

    begin_account_deletion(port.as_ref(), &command, now)
        .await
        .expect("accepted command");

    assert_eq!(
        *port.calls.lock().expect("calls lock"),
        vec![(
            "verifier_1".into(),
            "acct_1".into(),
            "session_1".into(),
            now,
        )]
    );

    let mut stale = command.clone();
    stale.authenticated_at = now - Duration::minutes(11);
    assert!(matches!(
        begin_account_deletion(port.as_ref(), &stale, now).await,
        Err(OperationsError::ReauthenticationRequired)
    ));

    let mut wrong_phrase = command.clone();
    wrong_phrase.typed_phrase = "delete my account".into();
    assert!(matches!(
        begin_account_deletion(port.as_ref(), &wrong_phrase, now).await,
        Err(OperationsError::ConfirmationPhraseRejected)
    ));
    assert_eq!(port.calls.lock().expect("calls lock").len(), 1);
}
