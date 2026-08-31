//! Test-only bootstrap (ADR-0052, plan §5.8). NEVER
//! compiled without the `test-fixtures` feature.
//!
//! Black-box conformance suites need a way to provision
//! ready tenants without running the full async scheduler
//! (which lands in Task 6.2). The bootstrap env var
//! `MEMORY_MCP_HTTP_TEST_BOOTSTRAP` carries a comma-separated
//! list of `<name>=<api_key>` pairs. Each entry is parsed
//! through `ApiKeyCredential::parse`, and a deterministic
//! Account + Tenant + ApiKey triple is written to the
//! registry. The tenant is then transitioned to `Ready`
//! synchronously through `provision_one` against a stub
//! `ApplyMigrations` so the conformance suite sees a
//! `Ready` tenant without touching the real SurrealDB.

#![cfg(feature = "test-fixtures")]

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::HttpState;
use crate::http::leases::migration::{NoopMigrations, provision_one};
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
    let noop: Arc<dyn ApplyMigrations> = Arc::new(NoopMigrations);
    for entry in raw.split(',') {
        let (name, key) = entry.split_once('=').ok_or_else(|| {
            MemoryError::ConfigInvalid("bootstrap entry must be <name>=<api_key>".into())
        })?;
        let cred = ApiKeyCredential::parse(key)?;
        bootstrap_one(state, name, &cred, noop.clone()).await?;
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
        schema_version: 0,
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
    // implementation; NoopMigrations is a no-op so the
    // registry holds a Ready tenant without the real DDL
    // running.
    provision_one(store.clone(), &tenant.id, lease, migrations).await?;
    Ok(())
}
