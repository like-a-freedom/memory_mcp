//! Deterministic crash-and-recovery fault injection (ADR-0053, Task 6).
//!
//! `FaultInjector` is the seam the production composition wires into the
//! provisioning, task, outbox, and deletion workers. The `NoFaults`
//! placeholder is what the binary uses in production; tests construct
//! `FailOnceAt` (gated on `test-fixtures`) so they can drive a transient
//! error at a named durable transition and prove the next worker advances
//! the same state forward.
//!
//! The injector is **not** global state. It is owned by
//! `HttpProductionComposition` and threaded through the scheduler option
//! objects (`SchedulerHooks::with_provisioning_only`,
//! `RuntimeOptions`, and the deletion worker construction).

#[cfg(any(test, feature = "test-fixtures"))]
use std::sync::Arc;

use crate::error::MemoryError;

/// Named durable transitions that the recovery tests exercise.
///
/// Every variant corresponds to a point the implementation commits to the
/// registry before any caller-visible acknowledgement. The fault is meant
/// to fire **after** that commit so the next worker, on a fresh process,
/// sees the partial state and advances it forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint {
    /// `RegistryStore::claim_provisioning` returned a `ProvisioningLease`.
    ProvisioningLeaseClaimed,
    /// `ApplyMigrations::ensure_namespace` returned `Ok(())`.
    NamespaceCreated,
    /// `ApplyMigrations::apply_migrations` returned the new schema version.
    TenantMigrationsApplied,
    /// The fenced `Migrating → Ready` transition committed.
    TenantReadyCommitted,
    /// `TaskStore::claim_next_due` returned a `TaskHandle`.
    TaskClaimed,
    /// `DurableTaskStore::record_artifact_fenced` upserted the artifact row.
    TaskArtifactCommitted,
    /// `TaskStore::complete_fenced` set the terminal state.
    TaskCompleted,
    /// `commit_tenant_mutation_with_event` committed the outbox transaction.
    OutboxMutationCommitted,
    /// `RegistryStore::begin_account_deletion` returned `Ok(())`.
    AccountDeletionStarted,
    /// `RegistryStore::finalize_account_deletion` returned `Ok(())`.
    AccountDeletionFinalized,
}

impl FaultPoint {
    /// Parse a `FaultPoint` from the env-var name the test fixture sends.
    /// Used by the binary on startup to construct a `FailOnceAt` from
    /// `MEMORY_MCP_HTTP_TEST_FAULT_POINT`.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn from_env_name(name: &str) -> Option<Self> {
        match name {
            "ProvisioningLeaseClaimed" => Some(Self::ProvisioningLeaseClaimed),
            "NamespaceCreated" => Some(Self::NamespaceCreated),
            "TenantMigrationsApplied" => Some(Self::TenantMigrationsApplied),
            "TenantReadyCommitted" => Some(Self::TenantReadyCommitted),
            "TaskClaimed" => Some(Self::TaskClaimed),
            "TaskArtifactCommitted" => Some(Self::TaskArtifactCommitted),
            "TaskCompleted" => Some(Self::TaskCompleted),
            "OutboxMutationCommitted" => Some(Self::OutboxMutationCommitted),
            "AccountDeletionStarted" => Some(Self::AccountDeletionStarted),
            "AccountDeletionFinalized" => Some(Self::AccountDeletionFinalized),
            _ => None,
        }
    }
}

/// The fault-injection seam. Production composition uses [`NoFaults`];
/// tests use [`FailOnceAt`] (gated on `test-fixtures`).
pub trait FaultInjector: Send + Sync + 'static {
    /// Inspect the named transition point and either pass through
    /// (`Ok(())`) or return a transient error the worker treats as a
    /// crash boundary. The implementation MUST be cheap and MUST NOT
    /// panic; a panic would terminate the test runner.
    fn hit(&self, point: FaultPoint) -> Result<(), MemoryError>;
}

/// The default injector. Every transition passes through.
#[derive(Debug, Default)]
pub struct NoFaults;

impl FaultInjector for NoFaults {
    fn hit(&self, _point: FaultPoint) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// A `FaultInjector` that returns `MemoryError::Transient` exactly once at
/// the configured point, then passes through. Used by the crash-recovery
/// test suite to prove the next worker advances the partial state.
///
/// The state is shared via `Arc<AtomicUsize>` so the scheduler, the
/// provisioning worker, and the deletion worker all see the same counter.
#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug)]
pub struct FailOnceAt {
    point: FaultPoint,
    /// How many `hit` calls at `point` should return an error. Zero
    /// means pass through; tests default to `1`.
    fires: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    consumed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl FailOnceAt {
    /// Build a one-shot injector at the given point.
    pub fn new(point: FaultPoint) -> Self {
        Self {
            point,
            fires: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1)),
            consumed: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Build an injector that returns a transient error on the first
    /// `n` matching `hit` calls before passing through.
    pub fn with_fires(point: FaultPoint, n: usize) -> Self {
        Self {
            point,
            fires: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(n)),
            consumed: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Construct from the env-var pair the test fixture uses. Falls
    /// back to a never-firing injector (i.e. `NoFaults`) when the
    /// name is unset or unrecognised.
    pub fn from_env() -> Arc<dyn FaultInjector> {
        let Some(name) = std::env::var("MEMORY_MCP_HTTP_TEST_FAULT_POINT")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Arc::new(NoFaults);
        };
        let Some(point) = FaultPoint::from_env_name(&name) else {
            eprintln!("memory_mcp::http::fault_injection: unknown fault point {name}");
            return Arc::new(NoFaults);
        };
        let n: usize = std::env::var("MEMORY_MCP_HTTP_TEST_FAULT_AT")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(1);
        Arc::new(Self::with_fires(point, n))
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl FaultInjector for FailOnceAt {
    fn hit(&self, point: FaultPoint) -> Result<(), MemoryError> {
        if point != self.point {
            return Ok(());
        }
        let remaining = self.fires.load(std::sync::atomic::Ordering::Acquire);
        if remaining == 0 {
            return Ok(());
        }
        // Reserve one fire atomically: only the winner decrements and
        // returns the error. Losers see the previous `remaining` and
        // still return the error (consistent with "the first matching
        // hit returns Transient"), then the next caller sees
        // `remaining == 0` and passes through.
        if self
            .fires
            .compare_exchange(
                remaining,
                remaining.saturating_sub(1),
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            self.consumed
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            return Err(MemoryError::Transient(format!(
                "simulated transient at {point:?}"
            )));
        }
        // A concurrent caller already decremented; reload and decide.
        let after = self.fires.load(std::sync::atomic::Ordering::Acquire);
        if after == 0 {
            Ok(())
        } else {
            self.consumed
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Err(MemoryError::Transient(format!(
                "simulated transient at {point:?}"
            )))
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl FailOnceAt {
    /// How many times the injector has returned the simulated transient.
    /// Tests assert this matches the configured fire count.
    pub fn consumed(&self) -> usize {
        self.consumed.load(std::sync::atomic::Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_faults_always_passes() {
        let injector = NoFaults;
        for point in [
            FaultPoint::ProvisioningLeaseClaimed,
            FaultPoint::TenantReadyCommitted,
            FaultPoint::OutboxMutationCommitted,
        ] {
            assert!(injector.hit(point).is_ok());
        }
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    #[test]
    fn fail_once_at_fires_once_then_passes() {
        let injector = FailOnceAt::new(FaultPoint::TaskClaimed);
        // First hit at the configured point: error.
        assert!(matches!(
            injector.hit(FaultPoint::TaskClaimed),
            Err(MemoryError::Transient(_))
        ));
        // Subsequent hits at the same point pass.
        assert!(injector.hit(FaultPoint::TaskClaimed).is_ok());
        assert!(injector.hit(FaultPoint::TaskClaimed).is_ok());
        // Other points always pass.
        assert!(injector.hit(FaultPoint::TaskCompleted).is_ok());
        assert_eq!(injector.consumed(), 1);
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    #[test]
    fn fail_once_with_fires_n() {
        let injector = FailOnceAt::with_fires(FaultPoint::OutboxMutationCommitted, 3);
        for _ in 0..3 {
            assert!(matches!(
                injector.hit(FaultPoint::OutboxMutationCommitted),
                Err(MemoryError::Transient(_))
            ));
        }
        assert!(injector.hit(FaultPoint::OutboxMutationCommitted).is_ok());
        assert_eq!(injector.consumed(), 3);
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    #[test]
    fn fail_once_at_is_concurrency_safe() {
        use std::sync::Arc;
        use std::sync::atomic::Ordering;
        let injector = Arc::new(FailOnceAt::with_fires(
            FaultPoint::ProvisioningLeaseClaimed,
            1,
        ));
        let injector_for_task = injector.clone();
        let handle =
            std::thread::spawn(move || injector_for_task.hit(FaultPoint::ProvisioningLeaseClaimed));
        let r1 = injector.hit(FaultPoint::ProvisioningLeaseClaimed);
        let r2 = handle.join().unwrap();
        let errors = [r1.is_err() as u8, r2.is_err() as u8];
        // Exactly one of the two callers observes the error.
        assert_eq!(errors.iter().sum::<u8>(), 1);
        assert!(injector.hit(FaultPoint::ProvisioningLeaseClaimed).is_ok());
        assert_eq!(injector.consumed.load(Ordering::Acquire), 1);
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    #[test]
    fn env_name_round_trip() {
        for point in [
            FaultPoint::ProvisioningLeaseClaimed,
            FaultPoint::NamespaceCreated,
            FaultPoint::TenantMigrationsApplied,
            FaultPoint::TenantReadyCommitted,
            FaultPoint::TaskClaimed,
            FaultPoint::TaskArtifactCommitted,
            FaultPoint::TaskCompleted,
            FaultPoint::OutboxMutationCommitted,
            FaultPoint::AccountDeletionStarted,
            FaultPoint::AccountDeletionFinalized,
        ] {
            let name = format!("{point:?}");
            assert_eq!(FaultPoint::from_env_name(&name), Some(point));
        }
        assert_eq!(FaultPoint::from_env_name("NotAPoint"), None);
    }
}
