#![cfg(feature = "streamable-http")]

use std::sync::{Arc, Mutex};

use memory_mcp::MemoryError;
use memory_mcp::provisioning::api::{
    ApiKeyIssuancePort, ApiKeyOwner, CreateApiKeyCommand, NewApiKeyRecord, create_api_key,
};

type InsertedKey = (String, u32, Option<chrono::DateTime<chrono::Utc>>);

#[derive(Default)]
struct RecordingIssuance {
    owner: Mutex<Option<ApiKeyOwner>>,
    cap: Mutex<u32>,
    calls: Mutex<Vec<InsertedKey>>,
}

#[async_trait::async_trait]
impl ApiKeyIssuancePort for RecordingIssuance {
    async fn owner(&self, _account_id: &str) -> Result<Option<ApiKeyOwner>, MemoryError> {
        Ok(self.owner.lock().expect("owner lock").clone())
    }

    async fn active_key_cap(
        &self,
        _tenant_id: &str,
        plan_version: u32,
    ) -> Result<u32, MemoryError> {
        assert_eq!(plan_version, 1);
        Ok(*self.cap.lock().expect("cap lock"))
    }

    async fn insert_key(&self, key: NewApiKeyRecord) -> Result<(), MemoryError> {
        assert_eq!(key.name, "agent");
        assert!(key.id.starts_with("ak_"));
        assert_eq!(key.secret.len(), 64);
        assert!(key.expires_at.is_some());
        assert!(key.now >= chrono::Utc::now() - chrono::Duration::minutes(1));
        self.calls
            .lock()
            .expect("calls lock")
            .push((key.account_id, key.cap, key.expires_at));
        Ok(())
    }
}

#[tokio::test]
async fn api_key_issue_resolves_owner_cap_and_inserts_once() {
    let port = Arc::new(RecordingIssuance::default());
    *port.owner.lock().expect("owner lock") = Some(ApiKeyOwner {
        tenant_id: "ten_1".into(),
        plan_version: 1,
    });
    *port.cap.lock().expect("cap lock") = 5;
    let now = chrono::Utc::now();

    let created = create_api_key(
        port.as_ref(),
        CreateApiKeyCommand {
            account_id: "acct_1".into(),
            name: "agent".into(),
            expires_in_days: Some(7),
        },
        now,
    )
    .await
    .expect("issue key");

    assert!(created.id.starts_with("ak_"));
    assert_eq!(created.secret.len(), 64);
    assert_eq!(created.expires_at, Some(now + chrono::Duration::days(7)));
    assert_eq!(port.calls.lock().expect("calls lock").len(), 1);
}

#[tokio::test]
async fn api_key_issue_refuses_missing_account_without_inserting() {
    let port = RecordingIssuance::default();
    let error = create_api_key(
        &port,
        CreateApiKeyCommand {
            account_id: "missing".into(),
            name: "agent".into(),
            expires_in_days: None,
        },
        chrono::Utc::now(),
    )
    .await
    .expect_err("missing account");
    assert!(matches!(error, MemoryError::NotFound(_)));
    assert!(port.calls.lock().expect("calls lock").is_empty());
}
