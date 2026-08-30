//! Runtime lifecycle states (ADR-0052, plan §5.5).
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

use tokio::sync::broadcast;

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
    pub last_used: Instant,
    pub activation: ActivationSlot,
}

impl TenantRuntimeSlot {
    pub fn new() -> Self {
        Self {
            runtime: None,
            phase: RuntimePhase::Absent,
            pin_count: Arc::new(AtomicU32::new(0)),
            active_operations: AtomicU32::new(0),
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
