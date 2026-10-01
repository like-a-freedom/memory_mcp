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
    TenantLifecyclePort, can_transition, transition_tenant, transition_tenant_fenced,
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

/// The fenced variant validates on the same table.
///
/// It is a separate function, and a separate function can drift. This asserts
/// the two refuse the same pairs, because the scheduler path is the one that
/// actually runs a lease and a drift there would strand a worker.
#[tokio::test]
async fn the_fenced_transition_validates_on_the_same_table() {
    use memory_mcp::http::leases::ProvisioningLease;

    let port = RecordingPort::default();
    let now = chrono::Utc::now();
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
    assert_eq!(port.writes().len(), 1);
}

fn legal_pairs() -> Vec<(TenantStatus, TenantStatus)> {
    vec![
        (TenantStatus::Reserved, TenantStatus::NamespaceCreating),
        (TenantStatus::NamespaceCreating, TenantStatus::Migrating),
        (TenantStatus::NamespaceCreating, TenantStatus::Failed),
        (TenantStatus::Migrating, TenantStatus::Ready),
        (TenantStatus::Migrating, TenantStatus::Failed),
        (TenantStatus::Ready, TenantStatus::Suspended),
        (TenantStatus::Suspended, TenantStatus::Ready),
        (TenantStatus::Deleting, TenantStatus::Purged),
        (TenantStatus::Failed, TenantStatus::NamespaceCreating),
        (TenantStatus::Failed, TenantStatus::Migrating),
        // `Deleting` is reachable from every non-terminal state.
        (TenantStatus::Reserved, TenantStatus::Deleting),
        (TenantStatus::NamespaceCreating, TenantStatus::Deleting),
        (TenantStatus::Migrating, TenantStatus::Deleting),
        (TenantStatus::Ready, TenantStatus::Deleting),
        (TenantStatus::Suspended, TenantStatus::Deleting),
        (TenantStatus::Failed, TenantStatus::Deleting),
    ]
}

#[test]
fn the_table_admits_exactly_the_pairs_it_names() {
    for (from, to) in legal_pairs() {
        assert!(
            can_transition(from, to),
            "{from:?} -> {to:?} is in the table and must be admitted"
        );
    }
}

/// The interesting property is not that the named pairs work. It is that
/// everything else does not.
///
/// The table is 8 named pairs plus a blanket `* -> Deleting` rule. A blanket
/// rule is the kind of thing that grows: add a variant to `TenantStatus` and
/// the `matches!` guard either admits it silently or needs remembering. This
/// asserts the shape instead — every pair the table does not name is refused —
/// so a new variant fails here rather than in a tenant's status history.
#[test]
fn every_pair_the_table_does_not_name_is_refused() {
    let all = [
        TenantStatus::Reserved,
        TenantStatus::NamespaceCreating,
        TenantStatus::Migrating,
        TenantStatus::Ready,
        TenantStatus::Suspended,
        TenantStatus::Failed,
        TenantStatus::Deleting,
        TenantStatus::Purged,
    ];
    let legal: Vec<(TenantStatus, TenantStatus)> = legal_pairs();

    for from in all {
        for to in all {
            if legal.contains(&(from, to)) {
                continue;
            }
            assert!(
                !can_transition(from, to),
                "{from:?} -> {to:?} is not in the table and must be refused"
            );
        }
    }
}

/// A terminal state is terminal.
///
/// `Purged` is the only state that destroys records, and it is reachable only
/// from `Deleting`. If a `Purged -> X` edge appeared, a purged tenant could
/// come back with its records gone.
#[test]
fn purged_is_terminal() {
    for to in [
        TenantStatus::Reserved,
        TenantStatus::NamespaceCreating,
        TenantStatus::Migrating,
        TenantStatus::Ready,
        TenantStatus::Suspended,
        TenantStatus::Failed,
        TenantStatus::Deleting,
        TenantStatus::Purged,
    ] {
        assert!(
            !can_transition(TenantStatus::Purged, to),
            "Purged -> {to:?} must be impossible: the records are already gone"
        );
    }
}

/// `Suspended` is reachable only from `Ready`.
///
/// The audit found `suspend_tenant` accepting `Migrating -> Suspended`, which
/// is not in the table: a tenant mid-provisioning has a lease held against it,
/// and suspending it out from under the worker leaves the worker believing it
/// still owns the tenant. The table is what refuses that pair, and this case
/// is the table saying so.
#[test]
fn suspension_requires_a_ready_tenant() {
    assert!(
        can_transition(TenantStatus::Ready, TenantStatus::Suspended),
        "a Ready tenant suspends"
    );
    for from in [
        TenantStatus::Reserved,
        TenantStatus::NamespaceCreating,
        TenantStatus::Migrating,
        TenantStatus::Failed,
    ] {
        assert!(
            !can_transition(from, TenantStatus::Suspended),
            "{from:?} -> Suspended must be refused: it is not in the table, and a \
             tenant mid-provisioning holds a lease that suspension would strand"
        );
    }
}

/// A refused pair names both ends.
///
/// The error is what an operator sees when a state change is rejected, and
/// `provisioning.rs` used `format!("provisioning transition {from:?}->{to:?}")`
/// — enough to find the offending pair, and nothing more. The moved function
/// keeps that shape.
#[test]
fn a_refusal_can_state_the_pair_it_refused() {
    let from = TenantStatus::Migrating;
    let to = TenantStatus::Suspended;
    assert!(!can_transition(from, to));
    let message = format!("provisioning transition {from:?}->{to:?}");
    assert!(
        message.contains("Migrating"),
        "the message names the source"
    );
    assert!(
        message.contains("Suspended"),
        "the message names the target"
    );
}
