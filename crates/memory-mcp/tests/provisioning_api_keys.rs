#![cfg(feature = "streamable-http")]

use std::sync::{Arc, Mutex};

use memory_mcp::MemoryError;
use memory_mcp::http::principal::api_keys::{ApiKeyCredential, assemble_credential};
use memory_mcp::provisioning::api::{
    ApiKeyIssuancePort, ApiKeyOwner, CreateApiKeyCommand, NewApiKeyRecord, create_api_key,
};

type InsertedKey = (String, u32, Option<chrono::DateTime<chrono::Utc>>);

#[derive(Default)]
struct RecordingIssuance {
    owner: Mutex<Option<ApiKeyOwner>>,
    cap: Mutex<u32>,
    calls: Mutex<Vec<InsertedKey>>,
    /// When false, `insert_key` accepts a key with no expiry. The
    /// assertion follows the command rather than being hardcoded so a
    /// test can issue a non-expiring key.
    expect_expiry: Mutex<Option<bool>>,
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
        let expect_expiry = self
            .expect_expiry
            .lock()
            .expect("expect_expiry lock")
            .unwrap_or(true);
        assert_eq!(key.expires_at.is_some(), expect_expiry);
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

/// The secret is a one-time credential. `CreatedApiKey` must never
/// render it through `Debug`, so an accidental `{:?}` in a log line or
/// an error report cannot leak it. The value is still reachable through
/// explicit field access, which is what the issuing handler uses.
#[tokio::test]
async fn created_api_key_debug_redacts_the_one_time_secret() {
    let port = RecordingIssuance::default();
    *port.owner.lock().expect("owner lock") = Some(ApiKeyOwner {
        tenant_id: "ten_1".into(),
        plan_version: 1,
    });
    *port.cap.lock().expect("cap lock") = 5;
    *port.expect_expiry.lock().expect("expect_expiry lock") = Some(false);

    let created = create_api_key(
        &port,
        CreateApiKeyCommand {
            account_id: "acct_1".into(),
            name: "agent".into(),
            expires_in_days: None,
        },
        chrono::Utc::now(),
    )
    .await
    .expect("issue key");

    let rendered = format!("{created:?}");
    assert!(
        !rendered.contains(&created.secret),
        "Debug leaked the one-time secret: {rendered}"
    );
    assert!(rendered.contains("[REDACTED]"), "expected redaction marker");
    // The non-secret fields stay useful for diagnosis.
    assert!(rendered.contains(&created.id));
    assert_eq!(created.secret.len(), 64, "the secret itself is intact");
}

/// A key this workflow issues must be one the data plane accepts.
///
/// This asserts a property of the *id* the service mints: `ak_<uuid
/// v4>`, which is what the credential grammar requires. The credential
/// is assembled here by the shared helper, so this test cannot fail if
/// an adapter forgets to assemble it — it deliberately does not try to.
/// That gap is covered at the transport boundary by
/// `issued_api_key_secret_is_a_usable_data_plane_credential` and
/// `issued_api_key_credential_authenticates_against_the_data_plane` in
/// `http_control_plane`, which drive the real endpoint.
#[tokio::test]
async fn issued_credential_is_accepted_by_the_data_plane_parser() {
    let port = Arc::new(RecordingIssuance::default());
    *port.owner.lock().expect("owner lock") = Some(ApiKeyOwner {
        tenant_id: "ten_1".into(),
        plan_version: 1,
    });
    *port.cap.lock().expect("cap lock") = 5;
    *port.expect_expiry.lock().expect("expect_expiry lock") = Some(false);

    let created = create_api_key(
        port.as_ref(),
        CreateApiKeyCommand {
            account_id: "acct_1".into(),
            name: "agent".into(),
            expires_in_days: None,
        },
        chrono::Utc::now(),
    )
    .await
    .expect("issue key");

    let credential = assemble_credential(&created.id, &created.secret);
    let parsed = ApiKeyCredential::parse(&credential).unwrap_or_else(|error| {
        panic!("an issued key must be usable: {credential} was refused ({error:?})")
    });
    assert_eq!(
        parsed.key_id(),
        created.id,
        "the credential must resolve back to the id that was stored"
    );
}

/// `CreatedApiKey.secret` must stay the bare token the registry's
/// verifier is computed over, and must not become the assembled
/// credential.
///
/// These are the two halves of a contract that is easy to "fix" in the
/// wrong place. Assembling inside `create_api_key` looks tidier and is
/// wrong: `insert_key` then stores `HMAC(pepper, "mem_sk_ak_…_…")`,
/// while the data plane recomputes the HMAC over the *tail* of the
/// credential the client presents — so every issued key would parse
/// and then 401, turning a visible failure into a silent one. The
/// assembly belongs in the adapter that returns the secret.
#[tokio::test]
async fn the_issued_secret_is_the_bare_verifier_input() {
    let port = Arc::new(RecordingIssuance::default());
    *port.owner.lock().expect("owner lock") = Some(ApiKeyOwner {
        tenant_id: "ten_1".into(),
        plan_version: 1,
    });
    *port.cap.lock().expect("cap lock") = 5;
    *port.expect_expiry.lock().expect("expect_expiry lock") = Some(false);

    let created = create_api_key(
        port.as_ref(),
        CreateApiKeyCommand {
            account_id: "acct_1".into(),
            name: "agent".into(),
            expires_in_days: None,
        },
        chrono::Utc::now(),
    )
    .await
    .expect("issue key");

    assert!(
        !created.secret.starts_with("mem_sk_"),
        "the service must return the bare secret; the adapter assembles the credential"
    );
    assert!(
        ApiKeyCredential::parse(&created.secret).is_err(),
        "the bare secret must not itself be a parseable credential, or this test would pass for the wrong reason"
    );
}
