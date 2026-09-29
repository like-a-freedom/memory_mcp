//! Runtime lifecycle states.
//!
//! Each `TenantRuntimeSlot` carries a `RuntimePhase` that the
//! pool mutates under a per-tenant mutex. The transitions
//! are: `Absent -> Loading -> Ready`, `Ready -> Draining ->
//! Unloaded` (eviction), and any state can short-circuit to
//! `Failed` on activation error.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tokio::sync::{Semaphore, broadcast};

use super::storage::TenantRuntime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePhase {
    Absent,
    Loading,
    Ready,
    Draining,
    Unloaded,
    Failed,
}

/// In-flight activation slot. `acquire_or_wait` subscribes to
/// the broadcast channel so all callers wait on the SAME
/// activation; the producer (the worker that wins the race)
/// sends the runtime on every receiver.
pub struct ActivationSlot {
    pub state: RuntimePhase,
    /// Increments on every (re)activation. Used to discard
    /// broadcasts from a previous activation that a slow
    /// subscriber might still receive.
    pub generation: AtomicU64,
    pub in_flight: Option<broadcast::Sender<Arc<TenantRuntime>>>,
    /// If a recent activation failed, future activations
    /// short-circuit until this Instant passes.
    pub negative_backoff_until: Option<Instant>,
}

impl Default for ActivationSlot {
    fn default() -> Self {
        Self {
            state: RuntimePhase::Absent,
            generation: AtomicU64::new(0),
            in_flight: None,
            negative_backoff_until: None,
        }
    }
}

impl ActivationSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new activation: bump the generation and store
    /// the broadcast sender. Subsequent `acquire_or_wait`
    /// calls subscribe to it.
    pub fn begin(&mut self) -> broadcast::Receiver<Arc<TenantRuntime>> {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = broadcast::channel(1);
        self.in_flight = Some(tx);
        rx
    }

    /// True if a previous activation failed and the backoff
    /// window is still open.
    pub fn in_negative_backoff(&self) -> bool {
        self.negative_backoff_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false)
    }
}

/// Per-Tenant slot. The LRU pool maps `tenant_id` to
/// `Arc<Mutex<TenantRuntimeSlot>>`. The slot's `phase` and
/// `last_used` drive the idle-eviction tick.
pub struct TenantRuntimeSlot {
    pub runtime: Option<Arc<TenantRuntime>>,
    pub phase: RuntimePhase,
    pub pin_count: Arc<AtomicU32>,
    pub active_operations: AtomicU32,
    pub concurrency: Arc<Semaphore>,
    pub last_used: Instant,
    pub activation: ActivationSlot,
}

impl TenantRuntimeSlot {
    pub fn new() -> Self {
        Self::new_with_limit(4)
    }

    pub fn new_with_limit(limit: u32) -> Self {
        Self {
            runtime: None,
            phase: RuntimePhase::Absent,
            pin_count: Arc::new(AtomicU32::new(0)),
            active_operations: AtomicU32::new(0),
            concurrency: Arc::new(Semaphore::new(limit.max(1) as usize)),
            last_used: Instant::now(),
            activation: ActivationSlot::new(),
        }
    }

    /// Pin the slot for an in-flight request. Returns the new
    /// pin count.
    pub fn pin(&mut self) -> u32 {
        self.last_used = Instant::now();
        self.pin_count.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Unpin. Returns the new count.
    pub fn unpin(&self) -> u32 {
        self.pin_count.fetch_sub(1, Ordering::SeqCst) - 1
    }
}

impl Default for TenantRuntimeSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_fresh_slot_has_no_runtime() {
        assert!(TenantRuntimeSlot::new().runtime.is_none());
    }

    #[test]
    fn a_fresh_slot_is_absent() {
        assert_eq!(TenantRuntimeSlot::new().phase, RuntimePhase::Absent);
    }

    #[test]
    fn a_fresh_slot_has_no_pins() {
        assert_eq!(TenantRuntimeSlot::new().pin_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_fresh_slot_has_no_active_operations() {
        assert_eq!(
            TenantRuntimeSlot::new()
                .active_operations
                .load(Ordering::SeqCst),
            0
        );
    }

    #[test]
    fn pinning_increments_the_pin_count() {
        let mut slot = TenantRuntimeSlot::new();

        assert_eq!(slot.pin(), 1);
    }

    #[test]
    fn pinning_twice_reports_two_pins() {
        let mut slot = TenantRuntimeSlot::new();
        slot.pin();

        assert_eq!(slot.pin(), 2);
    }

    #[test]
    fn unpinning_decrements_the_pin_count() {
        let mut slot = TenantRuntimeSlot::new();
        slot.pin();

        assert_eq!(slot.unpin(), 0);
    }

    #[test]
    fn a_zero_concurrency_limit_is_raised_to_one() {
        let slot = TenantRuntimeSlot::new_with_limit(0);

        assert_eq!(slot.concurrency.available_permits(), 1);
    }

    #[test]
    fn the_default_slot_allows_four_concurrent_operations() {
        assert_eq!(TenantRuntimeSlot::new().concurrency.available_permits(), 4);
    }

    #[test]
    fn an_explicit_concurrency_limit_is_honoured() {
        assert_eq!(
            TenantRuntimeSlot::new_with_limit(7)
                .concurrency
                .available_permits(),
            7
        );
    }

    #[test]
    fn a_fresh_activation_slot_is_absent() {
        assert_eq!(ActivationSlot::new().state, RuntimePhase::Absent);
    }

    #[test]
    fn a_fresh_activation_slot_starts_at_generation_zero() {
        assert_eq!(ActivationSlot::new().generation.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn beginning_an_activation_increments_the_generation() {
        let mut slot = ActivationSlot::new();

        slot.begin();

        assert_eq!(slot.generation.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn beginning_a_second_activation_increments_the_generation_again() {
        let mut slot = ActivationSlot::new();
        slot.begin();

        slot.begin();

        assert_eq!(slot.generation.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn beginning_an_activation_records_the_broadcast_sender() {
        let mut slot = ActivationSlot::new();

        slot.begin();

        assert!(slot.in_flight.is_some());
    }

    #[test]
    fn a_fresh_slot_is_not_in_negative_backoff() {
        assert!(!ActivationSlot::new().in_negative_backoff());
    }

    #[test]
    fn a_slot_with_a_future_backoff_is_in_negative_backoff() {
        let mut slot = ActivationSlot::new();
        slot.negative_backoff_until = Some(Instant::now() + Duration::from_secs(60));

        assert!(slot.in_negative_backoff());
    }

    #[test]
    fn a_slot_whose_backoff_has_passed_is_no_longer_backed_off() {
        let mut slot = ActivationSlot::new();
        slot.negative_backoff_until = Some(Instant::now() - Duration::from_secs(1));

        assert!(!slot.in_negative_backoff());
    }
}
