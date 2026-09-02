//! Test-only bootstrap. NEVER
//! compiled without the `test-fixtures` feature.
//!
//! Black-box conformance suites need a way to provision
//! ready tenants without running the full async scheduler.
//! The bootstrap env var
//! `MEMORY_MCP_HTTP_TEST_BOOTSTRAP` carries a comma-separated
//! list of `<name>=<api_key>` pairs. Each entry is parsed
//! through `ApiKeyCredential::parse`, and a deterministic
//! Account + Tenant + ApiKey triple is written to the
//! already-selected registry state. The tenant is then
//! transitioned to `Ready` synchronously through
//! `provision_one` against the state's own tenant engine.
//! This module only seeds data against the composition the
//! process already selected; it never creates or swaps
//! Registry adapters (ADR-0053).

#![cfg(feature = "test-fixtures")]

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::HttpState;
use crate::http::leases::migration::{SurrealTenantMigrations, provision_one};
use crate::http::leases::{ProvisioningLease, migration::ApplyMigrations};
use crate::http::principal::api_keys::ApiKeyCredential;
use crate::http::registry::models::*;

pub const ENV_TEST_BOOTSTRAP: &str = "MEMORY_MCP_HTTP_TEST_BOOTSTRAP";

/// Idempotently provision the ready tenants listed in the
/// env var. Returns `Ok(())` when the env var is unset, the
/// value is empty, or every entry is already provisioned.
pub async fn apply_test_bootstrap(state: &Arc<HttpState>) -> Result<(), MemoryError> {
    let Some(raw) = std::env::var(ENV_TEST_BOOTSTRAP)
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        return Ok(());
    };
    apply_bootstrap_entries(state, &raw).await
}

/// Seed the `<name>=<api_key>` entries against the state's
/// already-selected adapters. Split out from
/// `apply_test_bootstrap` so tests can call it without
/// mutating the process environment.
pub async fn apply_bootstrap_entries(state: &Arc<HttpState>, raw: &str) -> Result<(), MemoryError> {
    let migrations: Arc<dyn ApplyMigrations> = Arc::new(SurrealTenantMigrations::new(
        state.registry.tenant_engine()?,
    ));
    for entry in raw.split(',') {
        let (name, key) = entry.split_once('=').ok_or_else(|| {
            MemoryError::ConfigInvalid("bootstrap entry must be <name>=<api_key>".into())
        })?;
        let cred = ApiKeyCredential::parse(key)?;
        bootstrap_one(state, name, &cred, migrations.clone()).await?
    }
    Ok(())
}

async fn bootstrap_one(
    state: &Arc<HttpState>,
    name: &str,
    cred: &ApiKeyCredential,
    migrations: Arc<dyn ApplyMigrations>,
) -> Result<(), MemoryError> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(MemoryError::ConfigInvalid(
            "test bootstrap account name must be alphanumeric/underscore".into(),
        ));
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(name.as_bytes());
    let suffix = hex::encode(&digest[..8]);
    let account_id = format!("acct_test_{suffix}");
    let tenant_id = format!("ten_test_{suffix}");
    let tenant_namespace = format!("tns_test_{suffix}");
    let now = chrono::Utc::now();
    let store = state.registry.store_clone();
    let account = Account {
        id: account_id.clone(),
        status: AccountStatus::Active,
        tenant_id: tenant_id.clone(),
        created_at: now,
    };
    let mut tenant = Tenant {
        id: tenant_id.clone(),
        status: TenantStatus::Reserved,
        namespace_binding: NamespaceBinding {
            namespace: tenant_namespace,
            database: "memory".into(),
        },
        plan_version: 1,
        // The conformance suite's bootstrap tenant must
        // sit inside REPLICA_SCHEMA_RANGE or the binary's
        // N/N-1 gate bounces it from Ready. A real
        // Reserved tenant is N-1 waiting to migrate.
        schema_version: crate::http::leases::migration::CURRENT_SCHEMA_VERSION.saturating_sub(1),
        retry_stage: None,
        provisioning_lease: None,
        created_at: now,
        version: 0,
    };
    let api_key = ApiKey {
        id: cred.key_id().to_string(),
        account_id: account_id.clone(),
        name: format!("test-bootstrap-{name}"),
        verifier: KeyedVerifier::compute(state.config.api_key_pepper.as_bytes(), cred.secret()),
        status: ApiKeyStatus::Active,
        created_at: now,
        expires_at: None,
        last_used_at: None,
        version: 0,
    };
    store.write_account(&account).await?;
    store.write_tenant(&tenant).await?;
    store.write_api_key(&api_key).await?;

    // Seed a lease so provision_one can advance from
    // Reserved to Ready without external claim.
    let lease = ProvisioningLease {
        owner_id: "test-bootstrap".into(),
        lease_id: format!("lease-{tenant_id}"),
        fencing_generation: 0,
        expires_at: now + chrono::Duration::seconds(60),
        heartbeat_at: now,
    };
    tenant.provisioning_lease = Some(ProvisioningLeaseState {
        owner_id: lease.owner_id.clone(),
        lease_id: lease.lease_id.clone(),
        expires_at: lease.expires_at,
        fencing_generation: lease.fencing_generation,
        heartbeat_at: lease.heartbeat_at,
    });
    store.write_tenant(&tenant).await?;

    // provision_one is generic over the migrations
    // implementation; the caller supplies the adapter that the
    // composition selected, so seeding proves the same state
    // machine the scheduler would run.
    provision_one(store.clone(), &tenant.id, lease, migrations).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::test_state::HttpStateTestBuilder;

    const UNIT_KEY: &str =
        "mem_sk_ak_aaaa0000-0000-4000-8000-000000000000_isolationtest0000000000000000000";

    #[tokio::test]
    async fn seeding_uses_the_explicitly_composed_state() {
        let state = HttpStateTestBuilder::new()
            .await
            .build()
            .await
            .expect("composed test state");
        apply_bootstrap_entries(&state, &format!("unit_one={UNIT_KEY}"))
            .await
            .expect("bootstrap entries");
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(b"unit_one");
        let suffix = hex::encode(&digest[..8]);
        let tenant = state
            .registry
            .store_clone()
            .find_tenant_by_id(&format!("ten_test_{suffix}"))
            .await
            .expect("tenant lookup")
            .expect("bootstrapped tenant exists");
        assert_eq!(tenant.status, TenantStatus::Ready);
    }
}
