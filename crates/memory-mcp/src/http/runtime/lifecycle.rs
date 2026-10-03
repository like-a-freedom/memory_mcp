//! Runtime lifecycle states.
//!
//! One slot per tenant, one state machine. The pool holds the slot's state and
//! mutates it under its own short-held lock; the transitions are
//! `Absent -> Loading -> Ready`, `Ready -> Draining` (eviction), and
//! `Loading -> Absent | Failed` when an attempt ends.
//!
//! The generation counter is what makes an attempt identifiable. A slot can
//! return to `Absent` and start loading again while an older attempt is still
//! finishing, and the older attempt must not publish into the newer one — that
//! is the bug this counter exists to prevent, not a broadcast to filter.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use tokio::sync::{Semaphore, watch};

use super::storage::TenantRuntime;
use crate::error::MemoryError;
use crate::tenancy::api::TenantRuntimeIdentity;

/// How long a failed activation keeps a tenant in negative backoff.
pub(super) const ACTIVATION_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

/// The runtime inputs a slot was built from.
///
/// Distinct from [`TenantRuntimeIdentity`] on purpose. A change here is a
/// runtime the pool should replace; a change there is a different binding,
/// which the pool must refuse. Lifecycle status appears in neither: it is
/// resolved per request and is never a reason to reuse or replace a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeRevision {
    pub plan_version: u32,
    pub schema_version: u32,
    pub concurrency: u32,
}

/// What one attempt reports to the callers waiting on it.
///
/// The runtime itself is deliberately not carried: a follower reads it from the
/// slot under the pool's lock, so a runtime can never be paired with the state
/// of a different slot.
pub type ActivationOutcome = Result<(), MemoryError>;

/// What a slot is doing now.
pub enum SlotState {
    /// No runtime, and none being built.
    Absent,
    /// An attempt is running. The channel reports its outcome.
    Loading,
    /// A resident runtime, ready to serve.
    Ready { runtime: Arc<TenantRuntime> },
    /// The last attempt failed; the slot is refused until `retry_at`.
    Failed { retry_at: Instant },
    /// Being unloaded: still resident for the guards that hold it, no longer
    /// serving new acquisitions.
    Draining { runtime: Arc<TenantRuntime> },
}

/// Per-tenant slot: the pool's whole record of what it holds for a tenant.
pub struct TenantRuntimeSlot {
    /// Which tenant and which storage this slot is bound to.
    pub identity: TenantRuntimeIdentity,
    /// The revision the resident runtime was built for.
    pub revision: RuntimeRevision,
    /// Bumped when an attempt starts. Identifies that attempt.
    pub generation: u64,
    pub state: SlotState,
    /// Present only while `Loading`. Dropping it closes the channel, which is
    /// how a follower learns the attempt was abandoned rather than answering.
    pub completion: Option<watch::Sender<Option<ActivationOutcome>>>,
    pub pins: Arc<AtomicU32>,
    pub concurrency: Arc<Semaphore>,
    pub last_used: Instant,
}

impl TenantRuntimeSlot {
    pub fn new(identity: TenantRuntimeIdentity, revision: RuntimeRevision) -> Self {
        Self {
            identity,
            revision,
            generation: 0,
            state: SlotState::Absent,
            completion: None,
            pins: Arc::new(AtomicU32::new(0)),
            concurrency: Arc::new(Semaphore::new(revision.concurrency.max(1) as usize)),
            last_used: Instant::now(),
        }
    }

    /// The resident runtime, when this slot is ready to serve.
    pub fn ready_runtime(&self) -> Option<&Arc<TenantRuntime>> {
        match &self.state {
            SlotState::Ready { runtime } => Some(runtime),
            _ => None,
        }
    }

    /// Whether a recent attempt failed inside its backoff window.
    pub fn in_negative_backoff(&self, now: Instant) -> bool {
        matches!(&self.state, SlotState::Failed { retry_at } if *retry_at > now)
    }

    /// Whether this slot may be unloaded to make room.
    ///
    /// An attempt in flight is never reclaimable, and neither is a slot a
    /// response still holds: dropping the runtime under a live guard is how a
    /// pooled runtime disappears mid-request.
    pub fn is_reclaimable(&self, now: Instant, idle_ttl: std::time::Duration) -> bool {
        if self.pins.load(Ordering::SeqCst) > 0 {
            return false;
        }
        match &self.state {
            SlotState::Absent => true,
            SlotState::Failed { retry_at } => *retry_at <= now,
            SlotState::Ready { .. } => now.duration_since(self.last_used) >= idle_ttl,
            SlotState::Loading | SlotState::Draining { .. } => false,
        }
    }

    /// Start an attempt: bump the generation, publish `Loading`, and return the
    /// receiver its followers wait on.
    pub fn begin_loading(&mut self) -> (u64, watch::Receiver<Option<ActivationOutcome>>) {
        self.generation += 1;
        let (sender, receiver) = watch::channel(None);
        self.state = SlotState::Loading;
        self.completion = Some(sender);
        self.last_used = Instant::now();
        (self.generation, receiver)
    }

    /// A receiver on the in-flight attempt, if one is running.
    pub fn subscribe(&self) -> Option<watch::Receiver<Option<ActivationOutcome>>> {
        self.completion.as_ref().map(watch::Sender::subscribe)
    }

    /// Complete a `Loading` attempt.
    ///
    /// Returns whether the outcome was applied. A stale attempt returns `false`
    /// and is dropped without touching the slot: it was cancelled or superseded,
    /// and its result describes a runtime nobody is waiting for any more.
    pub fn finish_attempt(
        &mut self,
        generation: u64,
        runtime: Result<Arc<TenantRuntime>, MemoryError>,
        now: Instant,
    ) -> bool {
        if self.generation != generation || !matches!(self.state, SlotState::Loading) {
            return false;
        }
        let succeeded = runtime.is_ok();
        self.state = match runtime {
            Ok(runtime) => SlotState::Ready { runtime },
            Err(_) => SlotState::Failed {
                retry_at: now + ACTIVATION_BACKOFF,
            },
        };
        // Wake the followers with an answer before closing the channel. A
        // sender dropped without a value is the cancellation signal, so
        // conflating the two would report every completed attempt as abandoned.
        if let Some(sender) = self.completion.take() {
            let _ = sender.send(Some(if succeeded {
                Ok(())
            } else {
                Err(MemoryError::Unavailable(
                    "tenant runtime activation failed".into(),
                ))
            }));
        }
        true
    }

    /// End a `Loading` attempt without an outcome: it was cancelled, or the
    /// server is shutting down. The slot becomes immediately retryable, which
    /// is the difference between a cancelled request and a failing activation.
    pub fn abandon(&mut self, generation: u64) -> bool {
        if self.generation != generation || !matches!(self.state, SlotState::Loading) {
            return false;
        }
        self.state = SlotState::Absent;
        self.completion = None;
        true
    }
}

/// `SlotState::Ready` is only ever built by the pool, which has the runtime in
/// hand when an attempt finishes.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenancy::api::TenantRuntimeSpec;

    fn identity(tenant_id: &str, namespace: &str) -> TenantRuntimeIdentity {
        TenantRuntimeSpec {
            tenant_id: tenant_id.to_string(),
            namespace: namespace.to_string(),
            database: "memory".to_string(),
            plan_version: 1,
            schema_version: 0,
            status: crate::tenancy::api::TenantLifecycleStatus::Ready,
        }
        .identity()
    }

    fn revision() -> RuntimeRevision {
        RuntimeRevision {
            plan_version: 1,
            schema_version: 0,
            concurrency: 4,
        }
    }

    fn slot() -> TenantRuntimeSlot {
        TenantRuntimeSlot::new(identity("ten_a", "tns_a"), revision())
    }

    #[test]
    fn a_fresh_slot_is_absent_with_no_runtime() {
        let slot = slot();
        assert!(matches!(slot.state, SlotState::Absent));
        assert!(slot.ready_runtime().is_none());
        assert_eq!(slot.generation, 0);
    }

    #[test]
    fn a_fresh_slot_has_no_pins_and_honours_its_concurrency() {
        let slot = slot();
        assert_eq!(slot.pins.load(Ordering::SeqCst), 0);
        assert_eq!(slot.concurrency.available_permits(), 4);
    }

    #[test]
    fn a_zero_concurrency_limit_is_raised_to_one() {
        let slot = TenantRuntimeSlot::new(
            identity("ten_a", "tns_a"),
            RuntimeRevision {
                concurrency: 0,
                ..revision()
            },
        );
        assert_eq!(slot.concurrency.available_permits(), 1);
    }

    #[test]
    fn beginning_an_attempt_bumps_the_generation_and_publishes_loading() {
        let mut slot = slot();
        let (generation, _rx) = slot.begin_loading();
        assert_eq!(generation, 1);
        assert!(matches!(slot.state, SlotState::Loading));
        assert!(slot.subscribe().is_some());
    }

    #[test]
    fn a_second_attempt_gets_a_new_generation() {
        let mut slot = slot();
        let (first, _rx) = slot.begin_loading();
        let (second, _rx) = slot.begin_loading();
        assert_ne!(first, second);
    }

    #[test]
    fn abandoning_an_attempt_makes_the_slot_retryable_without_backoff() {
        let mut slot = slot();
        let (generation, _rx) = slot.begin_loading();
        assert!(slot.abandon(generation));
        assert!(matches!(slot.state, SlotState::Absent));
        assert!(!slot.in_negative_backoff(Instant::now()));
        assert!(slot.completion.is_none());
    }

    #[test]
    fn a_stale_generation_cannot_abandon_a_newer_attempt() {
        let mut slot = slot();
        let (stale, _rx) = slot.begin_loading();
        let (current, _rx) = slot.begin_loading();
        assert!(!slot.abandon(stale));
        assert!(matches!(slot.state, SlotState::Loading));
        assert!(slot.abandon(current));
    }

    #[test]
    fn a_failed_attempt_opens_a_backoff_window() {
        let mut slot = slot();
        let (generation, _rx) = slot.begin_loading();
        let now = Instant::now();
        assert!(slot.finish_attempt(
            generation,
            Err(MemoryError::Unavailable("factory said no".into())),
            now
        ));
        assert!(slot.in_negative_backoff(now));
        assert!(!slot.in_negative_backoff(now + ACTIVATION_BACKOFF));
    }

    #[test]
    fn a_pinned_slot_is_never_reclaimable() {
        let slot = slot();
        let now = Instant::now();
        assert!(slot.is_reclaimable(now, std::time::Duration::ZERO));
        slot.pins.fetch_add(1, Ordering::SeqCst);
        assert!(!slot.is_reclaimable(
            now + std::time::Duration::from_secs(3600),
            std::time::Duration::ZERO
        ));
    }

    #[test]
    fn a_loading_slot_is_never_reclaimable() {
        let mut slot = slot();
        let _ = slot.begin_loading();
        assert!(!slot.is_reclaimable(
            Instant::now() + std::time::Duration::from_secs(3600),
            std::time::Duration::ZERO
        ));
    }
}
