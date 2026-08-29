//! Tenant Registry seam (ADR-0052). Phase 3 stub; real store in Phase 4.

/// Phase 3 stub handle. `ping()` always returns `true` because no
/// real store is wired yet. Phase 4 replaces this with a real
/// control-namespace-backed handle.
pub struct RegistryHandle;

impl RegistryHandle {
    pub async fn ping(&self) -> bool {
        true
    }
}
