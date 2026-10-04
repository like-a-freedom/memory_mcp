#![cfg(feature = "control-plane")]

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::identity::api::{
    IdentityError, LinkMode, VerifiedIdentityCommand, VerifiedIdentityLinkTransactions,
    link_verified_identity,
};

type LinkCall = (String, String, [u8; 32], String, LinkMode, DateTime<Utc>);

#[derive(Default)]
struct RecordingLinkTransactions {
    calls: Mutex<Vec<LinkCall>>,
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
        actor: &str,
        mode: LinkMode,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.calls.lock().expect("calls lock").push((
            account_id.to_string(),
            issuer.to_string(),
            subject_verifier,
            actor.to_string(),
            mode,
            now,
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

fn at(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("fixed timestamp")
        .with_timezone(&Utc)
}

#[tokio::test]
async fn linking_an_identity_already_owned_by_the_account_is_idempotent() {
    let now = at("2026-10-03T12:00:00Z");
    let transactions = RecordingLinkTransactions::default();
    *transactions.existing_account.lock().expect("existing lock") = Some("acct_one".into());

    link_verified_identity(&transactions, &command("acct_one", LinkMode::Add), now)
        .await
        .expect("same account replay");
    assert!(transactions.calls.lock().expect("calls lock").is_empty());
}

#[tokio::test]
async fn linking_an_identity_owned_by_another_account_is_refused() {
    let now = at("2026-10-03T12:00:00Z");
    let transactions = RecordingLinkTransactions::default();
    *transactions.existing_account.lock().expect("existing lock") = Some("acct_one".into());

    assert!(matches!(
        link_verified_identity(&transactions, &command("acct_two", LinkMode::Add), now).await,
        Err(IdentityError::IdentityHeldByAnotherAccount)
    ));
    assert!(transactions.calls.lock().expect("calls lock").is_empty());
}

#[tokio::test]
async fn an_unclaimed_identity_is_linked_with_the_supplied_command_and_time() {
    let now = at("2026-10-03T12:00:00Z");
    let transactions = RecordingLinkTransactions::default();

    link_verified_identity(&transactions, &command("acct_one", LinkMode::Replace), now)
        .await
        .expect("replace");
    assert_eq!(
        transactions.calls.lock().expect("calls lock").as_slice(),
        [(
            "acct_one".into(),
            "https://idp.example.com".into(),
            [0xA1; 32],
            "admin_root".into(),
            LinkMode::Replace,
            now,
        )]
    );
}
