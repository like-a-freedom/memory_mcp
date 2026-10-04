//! Deletion recovery integration scenarios.
//!
//! The recovery workflow is driven through its public use case, while namespace
//! binding uses a real SurrealDB Mem engine. The recording ports control faults;
//! they do not claim to verify SQL transactions.
#![cfg(feature = "control-plane")]

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::operations::api::{
    DELETION_LEASE_TTL_SECS, DeletionLease, DeletionRecoveryPort, RecoveryOutcome,
    RetainedTenantWork, TenantUnderDeletion, run_deletion_recovery,
};
use memory_mcp::storage::{BoundDbClient, SurrealDbClient};

#[derive(Clone, Default)]
struct RecoveryTrace(Arc<Mutex<Vec<String>>>);

impl RecoveryTrace {
    fn record(&self, step: &str) {
        self.0.lock().expect("trace lock").push(step.to_string());
    }

    fn steps(&self) -> Vec<String> {
        self.0.lock().expect("trace lock").clone()
    }
}

fn at(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("fixed timestamp")
        .with_timezone(&Utc)
}

/// Which step of the workflow should fail, so the failure policy can be tested
/// at each one rather than only at the end.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum FailAt {
    #[default]
    None,
    /// The write fails and nothing is committed.
    Tombstone,
    /// The write commits and *then* the call fails. This is the shape a
    /// replica crash after a durable write actually takes, and it is the one
    /// the re-check exists to absorb.
    TombstoneAfterCommit,
}

/// A recording port. Every workflow step is observable here, which is the
/// point: the workflow moved into the context, so the tests drive the steps
/// rather than a single `recover_tenant` call that used to hide all of them.
#[derive(Default)]
struct RecoveryPort {
    listed: Mutex<Vec<String>>,
    /// Shared with retained work so ordering crosses the two owning ports.
    trace: RecoveryTrace,
    purged: Mutex<Vec<String>>,
    fail_at: Mutex<FailAt>,
    released: Mutex<Vec<String>>,
    claims: Mutex<usize>,
    /// When false, `bind_tenant_namespace` fails. The tests that need the
    /// workflow to reach the sweep flip it on; the ones about the pre-sweep
    /// sequence leave it off and read the failure as the subject.
    succeed_bind: bool,
}

impl RecoveryPort {
    fn fail_at(&self, at: FailAt) {
        *self.fail_at.lock().expect("fail_at lock") = at;
    }

    fn record(&self, step: &str) {
        self.trace.record(step);
    }

    fn should_fail(&self, at: FailAt) -> bool {
        *self.fail_at.lock().expect("fail_at lock") == at
    }

    fn tenant(&self, tenant_id: &str) -> TenantUnderDeletion {
        TenantUnderDeletion {
            tenant_id: tenant_id.to_string(),
            namespace: format!("tns_{tenant_id}"),
            database: "memory".to_string(),
            purged: self
                .purged
                .lock()
                .expect("purged lock")
                .iter()
                .any(|id| id == tenant_id),
        }
    }
}

#[async_trait::async_trait]
impl DeletionRecoveryPort for RecoveryPort {
    async fn list_deleting_tenants(
        &self,
        limit: usize,
        _now: DateTime<Utc>,
    ) -> Result<Vec<String>, MemoryError> {
        assert_eq!(limit, 64, "the pass stays bounded");
        Ok(self.listed.lock().expect("listed lock").clone())
    }

    async fn find_tenant_for_recovery(
        &self,
        tenant_id: &str,
    ) -> Result<Option<TenantUnderDeletion>, MemoryError> {
        self.record(&format!("find:{tenant_id}"));
        if self
            .listed
            .lock()
            .expect("listed lock")
            .iter()
            .all(|id| id != tenant_id)
        {
            return Ok(None);
        }
        Ok(Some(self.tenant(tenant_id)))
    }

    async fn claim_deletion_lease(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        ttl_secs: i64,
    ) -> Result<Option<DeletionLease>, MemoryError> {
        assert_eq!(ttl_secs, DELETION_LEASE_TTL_SECS);
        assert!(!owner_id.is_empty());
        self.record(&format!("claim:{tenant_id}"));
        *self.claims.lock().expect("claims lock") += 1;
        Ok(Some(DeletionLease {
            owner_id: owner_id.to_string(),
            lease_id: lease_id.to_string(),
            fencing_generation: *self.claims.lock().expect("claims lock") as u64,
        }))
    }

    async fn bind_tenant_namespace(
        &self,
        tenant: &TenantUnderDeletion,
    ) -> Result<Arc<BoundDbClient>, MemoryError> {
        self.record(&format!("bind:{}", tenant.tenant_id));
        if !self.succeed_bind {
            return Err(MemoryError::Storage("no test engine wired".into()));
        }
        // A real `BoundDbClient` over an in-memory engine. The workflow never
        // queries through it in these tests — the sequencing, the fencing and
        // the failure policy are what is under test, and all three sit above
        // the store — but handing the sweep a genuine handle keeps the port's
        // shape honest instead of substituting `None` for the one thing a
        // caller cannot choose.
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem engine");
        db.use_ns(&tenant.namespace)
            .use_db(&tenant.database)
            .await
            .expect("bind");
        Ok(Arc::new(BoundDbClient::new(
            Arc::new(SurrealDbClient::from_prebound(
                db,
                &tenant.namespace,
                "error",
            )),
            tenant.namespace.clone(),
        )))
    }

    async fn write_deletion_tombstone(
        &self,
        tenant_id: &str,
        _lease: &DeletionLease,
        _now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.record(&format!("tombstone:{tenant_id}"));
        if self.should_fail(FailAt::Tombstone) {
            return Err(MemoryError::Transient(format!("tombstone {tenant_id}")));
        }
        // Commit first, then optionally fail: the durable state is correct
        // even though the caller is told it was not.
        self.purged
            .lock()
            .expect("purged lock")
            .push(tenant_id.to_string());
        if self.should_fail(FailAt::TombstoneAfterCommit) {
            return Err(MemoryError::Transient(format!("died after {tenant_id}")));
        }
        Ok(())
    }

    async fn is_tenant_purged(&self, tenant_id: &str) -> Result<bool, MemoryError> {
        Ok(self
            .purged
            .lock()
            .expect("purged lock")
            .iter()
            .any(|id| id == tenant_id))
    }

    async fn release_deletion_lease(
        &self,
        tenant_id: &str,
        _lease: &DeletionLease,
    ) -> Result<(), MemoryError> {
        self.record(&format!("release:{tenant_id}"));
        self.released
            .lock()
            .expect("released lock")
            .push(tenant_id.to_string());
        Ok(())
    }
}

/// A sweep that records the tenant it ran for, and can be told to fail.
#[derive(Default)]
struct RetainedWork {
    trace: RecoveryTrace,
    fail: bool,
}

impl RetainedWork {
    fn failing(trace: RecoveryTrace) -> Self {
        Self { trace, fail: true }
    }
}

#[async_trait::async_trait]
impl RetainedTenantWork for RetainedWork {
    async fn purge_retained_work(
        &self,
        tenant_id: &str,
        _bound_db: Arc<BoundDbClient>,
    ) -> Result<(), MemoryError> {
        if self.fail {
            return Err(MemoryError::Transient(format!("sweep {tenant_id}")));
        }
        self.trace.record(&format!("sweep:{tenant_id}"));
        Ok(())
    }
}

/// The happy path, end to end through every step.
///
/// This is the shape the previous test could not express: the workflow used to
/// live behind a single `recover_tenant` call on the port, so a test could
/// only assert that the call happened, not that the sweep ran before the
/// tombstone and that the tenant ended up terminal.
#[tokio::test]
async fn a_recovered_tenant_is_swept_then_tombstoned() {
    let now = at("2026-10-03T12:00:00Z");
    let trace = RecoveryTrace::default();
    let port = RecoveryPort {
        trace: trace.clone(),
        succeed_bind: true,
        ..Default::default()
    };
    let work = RetainedWork {
        trace,
        ..Default::default()
    };
    *port.listed.lock().expect("listed lock") = vec!["ten_ok".into()];

    run_deletion_recovery(&port, &work, "replica_a", now)
        .await
        .expect("a clean pass");

    assert_eq!(
        port.trace.steps(),
        vec![
            "find:ten_ok",
            "claim:ten_ok",
            "bind:ten_ok",
            "sweep:ten_ok",
            "tombstone:ten_ok"
        ],
        "sweep then tombstone, and no release on the success path"
    );
    assert!(
        port.is_tenant_purged("ten_ok")
            .await
            .expect("tombstone state")
    );
    assert!(
        port.released.lock().expect("released lock").is_empty(),
        "a successful pass keeps its lease until it expires"
    );
}

#[tokio::test]
async fn deletion_recovery_preserves_first_error_and_continues_remaining_tenants() {
    let now = at("2026-10-03T12:00:00Z");
    let trace = RecoveryTrace::default();
    let port = Arc::new(RecoveryPort {
        trace: trace.clone(),
        succeed_bind: true,
        ..Default::default()
    });
    let work = RetainedWork {
        trace: trace.clone(),
        ..Default::default()
    };
    *port.listed.lock().expect("listed lock") =
        vec!["ten_first".into(), "ten_second".into(), "ten_third".into()];
    port.fail_at(FailAt::Tombstone);

    let error = run_deletion_recovery(port.as_ref(), &work, "replica_a", now)
        .await
        .expect_err("first recovery failure");

    assert!(error.to_string().contains("tombstone ten_first"));
    assert_eq!(
        trace.steps(),
        vec![
            "find:ten_first",
            "claim:ten_first",
            "bind:ten_first",
            "sweep:ten_first",
            "tombstone:ten_first",
            "release:ten_first",
            "find:ten_second",
            "claim:ten_second",
            "bind:ten_second",
            "sweep:ten_second",
            "tombstone:ten_second",
            "release:ten_second",
            "find:ten_third",
            "claim:ten_third",
            "bind:ten_third",
            "sweep:ten_third",
            "tombstone:ten_third",
            "release:ten_third",
        ],
        "a failed tenant does not stop the remaining recovery attempts"
    );
}

#[tokio::test]
async fn deletion_recovery_returns_early_when_no_work_exists() {
    let now = at("2026-10-03T12:00:00Z");
    let port = RecoveryPort::default();
    let work = RetainedWork::default();

    run_deletion_recovery(&port, &work, "replica_a", now)
        .await
        .expect("empty pass");

    assert!(port.trace.steps().is_empty());
}

/// The sweep happens before the tombstone, and never after it.
///
/// The order is the whole disposability contract: a replica that dies between
/// the two steps leaves a tenant that is still claimable, so the next one
/// redoes the idempotent sweep. Writing the tombstone first would strand a
/// tenant whose namespace was never swept and whose lease nobody reclaims.
#[tokio::test]
async fn a_failed_bind_stops_before_the_sweep() {
    let now = at("2026-10-03T12:00:00Z");
    let trace = RecoveryTrace::default();
    // `succeed_bind` stays false, so the bind itself fails.
    let port = RecoveryPort {
        trace: trace.clone(),
        ..Default::default()
    };
    let work = RetainedWork {
        trace,
        ..Default::default()
    };
    *port.listed.lock().expect("listed lock") = vec!["ten_x".into()];

    let error = run_deletion_recovery(&port, &work, "replica_a", now)
        .await
        .expect_err("bind is not wired in this fake");

    assert!(error.to_string().contains("no test engine wired"));
    assert_eq!(
        port.trace.steps(),
        vec!["find:ten_x", "claim:ten_x", "bind:ten_x", "release:ten_x"],
        "a failed bind releases the lease so the next replica can retry"
    );
    assert!(
        !port.trace.steps().contains(&"tombstone:ten_x".to_string()),
        "a failed bind must stop before retained work and the tombstone"
    );
}

/// A failure in the sweep leaves the lease released, so the next replica can
/// reclaim it immediately rather than waiting out the TTL.
#[tokio::test]
async fn a_failed_sweep_releases_the_lease_for_the_next_replica() {
    let now = at("2026-10-03T12:00:00Z");
    let trace = RecoveryTrace::default();
    let port = RecoveryPort {
        trace: trace.clone(),
        succeed_bind: true,
        ..Default::default()
    };
    let work = RetainedWork::failing(trace.clone());
    *port.listed.lock().expect("listed lock") = vec!["ten_y".into()];

    let error = run_deletion_recovery(&port, &work, "replica_a", now)
        .await
        .expect_err("sweep fails");

    assert!(error.to_string().contains("sweep ten_y"));
    assert_eq!(
        trace.steps(),
        vec!["find:ten_y", "claim:ten_y", "bind:ten_y", "release:ten_y"]
    );
    assert_eq!(
        *port.released.lock().expect("released lock"),
        vec!["ten_y"],
        "the failing replica must not strand the lease"
    );
}

/// A tenant that reached its terminal state is reported purged and never
/// claimed, so a replayed recovery pass is a no-op rather than a second write.
#[tokio::test]
async fn an_already_purged_tenant_is_never_claimed() {
    let now = at("2026-10-03T12:00:00Z");
    let trace = RecoveryTrace::default();
    let port = RecoveryPort {
        trace: trace.clone(),
        ..Default::default()
    };
    let work = RetainedWork {
        trace: trace.clone(),
        ..Default::default()
    };
    *port.listed.lock().expect("listed lock") = vec!["ten_z".into()];
    *port.purged.lock().expect("purged lock") = vec!["ten_z".into()];

    run_deletion_recovery(&port, &work, "replica_a", now)
        .await
        .expect("replay is a no-op");

    let steps = trace.steps();
    assert_eq!(
        steps,
        vec!["find:ten_z"],
        "a purged tenant stops at the read"
    );
}

/// A failure that lands after the tenant became terminal is not an error.
///
/// The tombstone write is the last step, and the fault injector fires right
/// after it commits. The durable state is therefore correct even though the
/// call reported a failure; re-checking is what turns that back into a
/// successful pass rather than a retry that would find nothing left to do.
#[tokio::test]
async fn a_failure_after_the_tombstone_is_reported_as_purged() {
    let now = at("2026-10-03T12:00:00Z");
    let trace = RecoveryTrace::default();
    let port = RecoveryPort {
        trace: trace.clone(),
        succeed_bind: true,
        ..Default::default()
    };
    // `FailAt::Tombstone` makes the write succeed and then fail, which is
    // exactly the "committed, then died" case.
    port.fail_at(FailAt::TombstoneAfterCommit);
    *port.listed.lock().expect("listed lock") = vec!["ten_w".into()];

    run_deletion_recovery(
        &port,
        &RetainedWork {
            trace: trace.clone(),
            ..Default::default()
        },
        "replica_a",
        now,
    )
    .await
    .expect("a committed tombstone is not an error, however the call reported it");

    assert!(
        port.is_tenant_purged("ten_w")
            .await
            .expect("tombstone state")
    );
    assert!(
        port.released.lock().expect("released lock").is_empty(),
        "the lease is not released when the tenant is already terminal"
    );
}

#[test]
fn recovery_outcome_distinguishes_purged_replay_from_progress() {
    assert!(RecoveryOutcome::Purged.is_terminal());
    assert!(!RecoveryOutcome::Finalized.is_terminal());
}
