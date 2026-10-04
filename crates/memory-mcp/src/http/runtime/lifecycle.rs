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
