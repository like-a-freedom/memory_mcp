#![cfg(feature = "control-plane")]

use std::sync::Mutex;

use memory_mcp::MemoryError;
use memory_mcp::identity::api::{
    InvitationSession, InvitationSessionPort, ensure_invitation_session,
};

#[derive(Default)]
struct RecordingSessionPort {
    has_session: bool,
    issued: Mutex<Vec<String>>,
    fail_issue: bool,
}

#[async_trait::async_trait]
impl InvitationSessionPort for RecordingSessionPort {
    async fn has_valid_session(
        &self,
        _account_id: &str,
        _cookie: Option<&str>,
    ) -> Result<bool, MemoryError> {
        Ok(self.has_session)
    }

    async fn issue_session(&self, account_id: &str) -> Result<InvitationSession, MemoryError> {
        if self.fail_issue {
            return Err(MemoryError::Storage("session write failed".into()));
        }
        self.issued
            .lock()
            .expect("issued lock")
            .push(account_id.to_string());
        Ok(InvitationSession {
            cookie_value: "issued-cookie".into(),
        })
    }
}

#[tokio::test]
async fn invitation_session_preserves_existing_or_issues_exactly_once() {
    let existing = RecordingSessionPort {
        has_session: true,
        ..RecordingSessionPort::default()
    };
    assert_eq!(
        ensure_invitation_session(&existing, "acct_one", None)
            .await
            .expect("existing session"),
        None
    );
    assert!(existing.issued.lock().expect("issued lock").is_empty());

    let missing = RecordingSessionPort::default();
    let session = ensure_invitation_session(&missing, "acct_one", None)
        .await
        .expect("new session")
        .expect("session issued");
    assert_eq!(session.cookie_value, "issued-cookie");
    assert_eq!(
        *missing.issued.lock().expect("issued lock"),
        vec!["acct_one".to_string()]
    );
}

#[tokio::test]
async fn invitation_session_failure_is_reported_after_identity_commit() {
    let port = RecordingSessionPort {
        fail_issue: true,
        ..RecordingSessionPort::default()
    };
    let error = ensure_invitation_session(&port, "acct_one", None)
        .await
        .expect_err("session storage failure");
    assert!(error.to_string().contains("session write failed"));
    assert!(port.issued.lock().expect("issued lock").is_empty());
}
