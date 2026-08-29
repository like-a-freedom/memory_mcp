//! Provisioning lease types (ADR-0052, plan §6.2).
//!
//! Task 4.1 defines the trait surface; the actual scheduler
//! implementation lands in Task 6.2.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Fenced provisioning lease returned by `claim_provisioning`.
/// `fencing_generation` is the value tenants compare incoming
/// requests against; the scheduler increments it whenever it
/// takes over a previously-claimed lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningLease {
    pub owner_id: String,
    pub lease_id: String,
    pub fencing_generation: u64,
    pub expires_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
}
