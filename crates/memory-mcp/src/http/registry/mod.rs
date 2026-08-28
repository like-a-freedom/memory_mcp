//! Tenant Registry seam (ADR-0052). Phase 3 stub; real store in Phase 4.

/// Phase 3 stub handle. Phase 4 replaces the stub constructor with a
/// real control-namespace-backed handle; `ping` keeps this exact
/// signature.
#[derive(Clone)]
pub struct RegistryHandle {
    stub: bool,
}

impl RegistryHandle {
    /// Phase 3 stub: always reachable. Removed in Task 4.1 when the
    /// real store lands.
    pub fn stub() -> Self {
        Self { stub: true }
    }

    pub async fn ping(&self) -> bool {
        self.stub
    }
}
