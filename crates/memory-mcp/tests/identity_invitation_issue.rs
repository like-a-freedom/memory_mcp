#![cfg(feature = "control-plane")]

use std::sync::Mutex;

use memory_mcp::MemoryError;
use memory_mcp::identity::api::{
    IdentityError, IdentityInvitationCommand, IdentityInvitationPort, validate_identity_invitation,
};

#[derive(Default)]
struct RecordingInvitationPort {
    account_exists: Mutex<bool>,
    identity_count: Mutex<usize>,
}

#[async_trait::async_trait]
impl IdentityInvitationPort for RecordingInvitationPort {
    async fn account_exists(&self, _account_id: &str) -> Result<bool, MemoryError> {
        Ok(*self.account_exists.lock().expect("account lock"))
    }

    async fn identity_count(&self, _account_id: &str) -> Result<usize, MemoryError> {
        Ok(*self.identity_count.lock().expect("count lock"))
    }
}

fn command(account_id: &str, replace: bool) -> IdentityInvitationCommand {
    IdentityInvitationCommand {
        account_id: account_id.into(),
        invited_by: "admin_root".into(),
        replace,
    }
}

#[tokio::test]
async fn invitation_issue_checks_account_and_exact_one_replace() {
    let port = RecordingInvitationPort::default();
    *port.account_exists.lock().expect("account lock") = false;
    assert!(matches!(
        validate_identity_invitation(&port, &command("acct_missing", false)).await,
        Err(IdentityError::NotFound)
    ));

    *port.account_exists.lock().expect("account lock") = true;
    *port.identity_count.lock().expect("count lock") = 2;
    assert!(matches!(
        validate_identity_invitation(&port, &command("acct_one", true)).await,
        Err(IdentityError::LastIdentityOrConflict)
    ));

    *port.identity_count.lock().expect("count lock") = 1;
    validate_identity_invitation(&port, &command("acct_one", true))
        .await
        .expect("valid invitation");
}
