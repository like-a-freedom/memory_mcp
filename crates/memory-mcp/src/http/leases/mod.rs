//! Provisioning leases (ADR-0052, plan §6.2).
//!
//! `ProvisioningLease` is the durable claim returned by the
//! scheduler. The fenced CAS in `RegistryStore` matches
//! `(owner_id, lease_id, fencing_generation)` against the
//! stored lease before any state advance.
//!
//! Phase 5 ships the data type and a minimal heartbeat helper
//! used by the migration worker (`migration::provision_one`).
//! Task 6.1 generalizes the lease primitive (heartbeat budget,
//! owner-rotation, etc.) and Task 6.2 wires the scheduler loop
//! that calls `claim_provisioning` periodically.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod migration;

/// Fenced provisioning lease returned by
/// `RegistryStore::claim_provisioning`. `fencing_generation`
/// is the value tenants compare incoming requests against; the
/// scheduler increments it whenever it takes over a
/// previously-claimed lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningLease {
    pub owner_id: String,
    pub lease_id: String,
    pub fencing_generation: u64,
    pub expires_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
}

impl ProvisioningLease {
    /// Seconds remaining until the lease expires. May be
    /// negative if the scheduler has not yet noticed expiry.
    pub fn ttl_secs(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_seconds()
    }

    /// True if the lease has expired relative to `now`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.ttl_secs(now) <= 0
    }
}
