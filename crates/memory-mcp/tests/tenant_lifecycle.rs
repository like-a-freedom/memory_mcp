//! The Tenant lifecycle transition table, at the seam the context declares.
//!
//! These cases moved out of `http/registry/provisioning.rs` with the table
//! itself. What changed is that the rule is now reachable from a test outside
//! the HTTP adapter, and — more importantly — that `can_transition` is a
//! function a use case can call *before* it holds a store, so an illegal
//! pair can be refused without a round trip.

#![cfg(feature = "streamable-http")]

use std::sync::Mutex;

use memory_mcp::MemoryError;
use memory_mcp::models::registry::TenantStatus;
use memory_mcp::provisioning::api::{
    TenantLifecyclePort, transition_tenant, transition_tenant_fenced,
};

/// Counts the writes the port is asked for, so a test can assert that an
/// illegal pair never reached it.
#[derive(Default)]
struct RecordingPort {
    writes: Mutex<Vec<(String, u64, TenantStatus, TenantStatus)>>,
}

impl RecordingPort {
    fn writes(&self) -> Vec<(String, u64, TenantStatus, TenantStatus)> {
        self.writes.lock().expect("writes lock").clone()
    }
}

#[async_trait::async_trait]
impl TenantLifecyclePort for RecordingPort {
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
    ) -> Result<u64, MemoryError> {
        self.writes.lock().expect("writes lock").push((
            tenant_id.to_owned(),
            expected_version,
            from,
            to,
        ));
        Ok(expected_version + 1)
    }

    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
        _lease: &memory_mcp::http::leases::ProvisioningLease,
    ) -> Result<u64, MemoryError> {
        self.writes.lock().expect("writes lock").push((
            tenant_id.to_owned(),
            expected_version,
            from,
            to,
        ));
        Ok(expected_version + 1)
    }
}

/// An illegal pair is refused before the store is touched.
///
/// The store's compare-and-set is on `expected_version`, not on `from`. That
/// makes a caller's own check load-bearing rather than a belt-and-braces:
/// without it, `update_tenant_state` would happily write `Migrating -> Ready`
/// on a Tenant whose provisioning worker still holds a lease, and both the
/// operator and the worker would then believe the Tenant is ready.
///
/// The refusal happens *before* the port is called, so no version is consumed.
#[tokio::test]
async fn an_illegal_transition_is_refused_before_the_store_is_touched() {
    let port = RecordingPort::default();

    let error = transition_tenant(
        &port,
        "tenant-1",
        7,
        TenantStatus::Migrating,
        TenantStatus::Suspended,
    )
    .await
    .expect_err("Migrating -> Suspended is not in the table");

    assert!(
        matches!(error, MemoryError::Validation(_)),
        "an illegal pair is a programmer error, not a storage failure"
    );
    assert!(
        port.writes().is_empty(),
        "the store must not be reached: a write would consume version 7 even \
         though the transition was illegal"
    );
}

#[tokio::test]
async fn a_legal_transition_reaches_the_store_with_the_pair_it_was_given() {
    let port = RecordingPort::default();

    let version = transition_tenant(
        &port,
        "tenant-1",
        7,
        TenantStatus::Ready,
        TenantStatus::Suspended,
    )
    .await
    .expect("Ready -> Suspended is in the table");

    assert_eq!(version, 8, "the store's new version is returned unchanged");
    assert_eq!(
        port.writes(),
        vec![(
            "tenant-1".to_owned(),
            7,
            TenantStatus::Ready,
            TenantStatus::Suspended
        )],
        "the port receives the pair that was validated, not a rewritten one"
    );
}

#[tokio::test]
async fn an_illegal_fenced_transition_is_refused_before_the_store_is_touched() {
    use memory_mcp::http::leases::ProvisioningLease;

    let port = RecordingPort::default();
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z")
        .expect("fixed timestamp")
        .with_timezone(&chrono::Utc);
    let lease = ProvisioningLease {
        owner_id: "worker-1".to_owned(),
        lease_id: "lease-1".to_owned(),
        fencing_generation: 3,
        expires_at: now + chrono::Duration::minutes(1),
        heartbeat_at: now,
    };

    let error = transition_tenant_fenced(
        &port,
        "tenant-1",
        1,
        TenantStatus::Reserved,
        TenantStatus::Ready,
        &lease,
    )
    .await
    .expect_err("Reserved -> Ready is not in the table");
    assert!(matches!(error, MemoryError::Validation(_)));
    assert!(port.writes().is_empty());
}

#[tokio::test]
async fn a_legal_fenced_transition_reaches_the_store_with_its_validated_pair() {
    use memory_mcp::http::leases::ProvisioningLease;

    let port = RecordingPort::default();
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z")
        .expect("fixed timestamp")
        .with_timezone(&chrono::Utc);
    let lease = ProvisioningLease {
        owner_id: "worker-1".to_owned(),
        lease_id: "lease-1".to_owned(),
        fencing_generation: 3,
        expires_at: now + chrono::Duration::minutes(1),
        heartbeat_at: now,
    };
    let version = transition_tenant_fenced(
        &port,
        "tenant-1",
        1,
        TenantStatus::Reserved,
        TenantStatus::NamespaceCreating,
        &lease,
    )
    .await
    .expect("Reserved -> NamespaceCreating is in the table");
    assert_eq!(version, 2);
    assert_eq!(
        port.writes(),
        vec![(
            "tenant-1".to_owned(),
            1,
            TenantStatus::Reserved,
            TenantStatus::NamespaceCreating
        )]
    );
}

#[tokio::test]
async fn a_purged_tenant_cannot_return_to_ready() {
    let port = RecordingPort::default();

    let error = transition_tenant(
        &port,
        "tenant-1",
        9,
        TenantStatus::Purged,
        TenantStatus::Ready,
    )
    .await
    .expect_err("Purged is terminal");

    assert!(matches!(error, MemoryError::Validation(_)));
    assert!(port.writes().is_empty());
}
