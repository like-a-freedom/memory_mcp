#![cfg(feature = "control-plane")]

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::operations::api::{DeletionRecoveryPort, RecoveryOutcome, run_deletion_recovery};

#[derive(Default)]
struct RecoveryPort {
    listed: Mutex<Vec<String>>,
    recovered: Mutex<Vec<String>>,
    failures: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl DeletionRecoveryPort for RecoveryPort {
    async fn list_deleting_tenants(
        &self,
        limit: usize,
        _now: DateTime<Utc>,
    ) -> Result<Vec<String>, MemoryError> {
        assert_eq!(limit, 64);
        Ok(self.listed.lock().expect("listed lock").clone())
    }

    async fn recover_tenant(
        &self,
        tenant_id: &str,
        _now: DateTime<Utc>,
    ) -> Result<RecoveryOutcome, MemoryError> {
        self.recovered
            .lock()
            .expect("recovered lock")
            .push(tenant_id.to_string());
        if self
            .failures
            .lock()
            .expect("failures lock")
            .iter()
            .any(|value| value == tenant_id)
        {
            return Err(MemoryError::Transient(format!("recover {tenant_id}")));
        }
        Ok(RecoveryOutcome::Finalized)
    }
}

#[tokio::test]
async fn deletion_recovery_preserves_first_error_and_continues_remaining_tenants() {
    let now = Utc::now();
    let port = Arc::new(RecoveryPort::default());
    *port.listed.lock().expect("listed lock") =
        vec!["ten_first".into(), "ten_second".into(), "ten_third".into()];
    *port.failures.lock().expect("failures lock") = vec!["ten_second".into()];

    let error = run_deletion_recovery(port.as_ref(), now)
        .await
        .expect_err("first recovery failure");

    assert!(error.to_string().contains("recover ten_second"));
    assert_eq!(
        *port.recovered.lock().expect("recovered lock"),
        vec!["ten_first", "ten_second", "ten_third"]
    );

    port.failures.lock().expect("failures lock").clear();
    run_deletion_recovery(port.as_ref(), now)
        .await
        .expect("clean pass");
}

#[tokio::test]
async fn deletion_recovery_returns_early_when_no_work_exists() {
    let now = Utc::now();
    let port = RecoveryPort::default();

    run_deletion_recovery(&port, now).await.expect("empty pass");

    assert!(port.recovered.lock().expect("recovered lock").is_empty());
}

#[test]
fn recovery_outcome_distinguishes_purged_replay_from_progress() {
    assert!(RecoveryOutcome::Purged.is_terminal());
    assert!(!RecoveryOutcome::Finalized.is_terminal());
}
