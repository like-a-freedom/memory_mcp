#![cfg(feature = "control-plane")]

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::identity::api::{
    IdentityError, LinkMode, VerifiedIdentityCommand, VerifiedIdentityLinkTransactions,
    link_verified_identity,
};

#[derive(Default)]
struct RecordingLinkTransactions {
    calls: Mutex<Vec<(String, String, LinkMode)>>,
    existing_account: Mutex<Option<String>>,
    error: Mutex<Option<MemoryError>>,
}

#[async_trait::async_trait]
impl VerifiedIdentityLinkTransactions for RecordingLinkTransactions {
    async fn find_identity_account(
        &self,
        _issuer: &str,
        _subject_verifier: &[u8; 32],
    ) -> Result<Option<String>, MemoryError> {
        Ok(self.existing_account.lock().expect("existing lock").clone())
    }

    async fn link_verified_identity(
        &self,
        account_id: &str,
        issuer: &str,
        subject_verifier: [u8; 32],
        _actor: &str,
        mode: LinkMode,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        assert_eq!(issuer, "https://idp.example.com");
        assert_eq!(subject_verifier, [0xA1; 32]);
        let _ = now;
        self.calls.lock().expect("calls lock").push((
            account_id.to_string(),
            issuer.to_string(),
            mode,
        ));
        if let Some(error) = self.error.lock().expect("error lock").take() {
            return Err(error);
        }
        Ok(())
    }
}

fn command(account_id: &str, mode: LinkMode) -> VerifiedIdentityCommand {
    VerifiedIdentityCommand {
        account_id: account_id.into(),
        issuer: "https://idp.example.com".into(),
        subject_verifier: [0xA1; 32],
        actor: "admin_root".into(),
        mode,
    }
}

#[tokio::test]
async fn verified_identity_link_replays_for_same_account_and_refuses_cross_account() {
    let now = Utc::now();
    let transactions = Arc::new(RecordingLinkTransactions::default());
    *transactions.existing_account.lock().expect("existing lock") = Some("acct_one".into());

    link_verified_identity(
        transactions.as_ref(),
        &command("acct_one", LinkMode::Add),
        now,
    )
    .await
    .expect("same account replay");
    assert!(transactions.calls.lock().expect("calls lock").is_empty());

    assert!(matches!(
        link_verified_identity(
            transactions.as_ref(),
            &command("acct_two", LinkMode::Add),
            now
        )
        .await,
        Err(IdentityError::IdentityHeldByAnotherAccount)
    ));

    *transactions.existing_account.lock().expect("existing lock") = None;
    link_verified_identity(
        transactions.as_ref(),
        &command("acct_one", LinkMode::Replace),
        now,
    )
    .await
    .expect("replace");
    assert_eq!(
        *transactions.calls.lock().expect("calls lock"),
        vec![(
            "acct_one".into(),
            "https://idp.example.com".into(),
            LinkMode::Replace
        )]
    );
}
