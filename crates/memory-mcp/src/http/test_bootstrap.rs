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
//!
//! The companion env var
//! `MEMORY_MCP_HTTP_TEST_SEED_RESERVED` skips the
//! `provision_one` step so the in-process scheduler is the
//! one that advances the tenant. The crash-recovery test
//! uses this to force the scheduler through a faulted
//! transition and prove the next worker finishes the
//! migration.

#![cfg(feature = "test-fixtures")]

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::HttpState;
use crate::http::leases::migration::{SurrealTenantMigrations, provision_one};
use crate::http::leases::{ProvisioningLease, migration::ApplyMigrations};
use crate::http::principal::api_keys::ApiKeyCredential;
use crate::http::registry::models::*;

pub const ENV_TEST_BOOTSTRAP: &str = "MEMORY_MCP_HTTP_TEST_BOOTSTRAP";

/// Reserved-only seeding path used by the crash-recovery test
/// (Task 6). Skips the synchronous `provision_one` so the
/// scheduler is the one that advances the tenant.
pub const ENV_TEST_SEED_RESERVED: &str = "MEMORY_MCP_HTTP_TEST_SEED_RESERVED";

/// Black-box control-plane session seeding for Task 8.
///
/// Format: `<account_name>=<cookie_value>`. The bootstrap account
/// must already exist (typically because
/// `MEMORY_MCP_HTTP_TEST_BOOTSTRAP` was processed for the same
/// name). A new `ControlPlaneSession` is generated and persisted
/// with `state.registry.store_clone().store_session(...)`. The
/// middleware verifies the cookie by re-hashing the raw value with
/// `MEMORY_MCP_HTTP_SESSION_KEY`, so the test owns the raw value and
/// sends it as `Cookie: __Host-memory_mcp_session=<value>` on every
/// control-plane request. Multiple entries may be separated by
/// commas; the cookie value is opaque to the server.
#[cfg(feature = "control-plane")]
pub const ENV_TEST_SEED_SESSION: &str = "MEMORY_MCP_HTTP_TEST_SEED_SESSION";

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

/// Idempotently seed the named tenants as `Reserved` without
/// running `provision_one`. Returns `Ok(())` when the env var is
/// unset or empty.
pub async fn apply_test_seed_reserved(state: &Arc<HttpState>) -> Result<(), MemoryError> {
    let Some(raw) = std::env::var(ENV_TEST_SEED_RESERVED)
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        return Ok(());
    };
    for entry in raw.split(',') {
        let (name, key) = entry.split_once('=').ok_or_else(|| {
            MemoryError::ConfigInvalid("seed entry must be <name>=<api_key>".into())
        })?;
        let cred = ApiKeyCredential::parse(key)?;
        seed_reserved_one(state, name, &cred).await?;
    }
    Ok(())
}

/// Idempotently seed control-plane sessions for the named
/// bootstrap accounts. Format: `<account_name>=<cookie_value>`
/// pairs separated by commas. Used by Task 8 control-plane
/// black-box tests so they can drive `/api/v1/account/*` without
/// the OIDC callback flow.
#[cfg(feature = "control-plane")]
pub async fn apply_test_seed_session(state: &Arc<HttpState>) -> Result<(), MemoryError> {
    let Some(raw) = std::env::var(ENV_TEST_SEED_SESSION)
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        return Ok(());
    };
    for entry in raw.split(',') {
        let (name, cookie) = entry.split_once('=').ok_or_else(|| {
            MemoryError::ConfigInvalid(
                "session seed entry must be <account_name>=<cookie_value>".into(),
            )
        })?;
        seed_session_one(state, name, cookie).await?;
    }
    Ok(())
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
    // Idempotent: a previous bootstrap that already reached
    // Ready means the recovery test restarted the fixture
    // against the same rocksdb path; do NOT regress.
    if let Some(existing) = store.find_tenant_by_id(&tenant_id).await?
        && existing.status == TenantStatus::Ready
    {
        return Ok(());
    }
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
    // machine the scheduler would run. The test bootstrap is
    // allowed to advance without fault injection: it is a
    // synchronous seeding path, not a recovery scenario.
    provision_one(
        store.clone(),
        &tenant.id,
        lease,
        migrations,
        Arc::new(crate::http::fault_injection::NoFaults),
    )
    .await?;
    Ok(())
}

/// Seed an Account + Tenant + ApiKey triple for `name` and
/// leave the tenant in `Reserved` so the scheduler is the one
/// that drives the provisioning transition.
async fn seed_reserved_one(
    state: &Arc<HttpState>,
    name: &str,
    cred: &ApiKeyCredential,
) -> Result<(), MemoryError> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(MemoryError::ConfigInvalid(
            "test seed account name must be alphanumeric/underscore".into(),
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
    let existing = store.find_tenant_by_id(&tenant_id).await?;
    if let Some(existing) = existing
        && existing.status == TenantStatus::Ready
    {
        // Idempotent: a previous run already advanced this
        // tenant to Ready. The crash-recovery test re-seeds
        // after `fixture.restart()`; we must not regress
        // a Ready tenant back to Reserved.
        return Ok(());
    }
    let account = Account {
        id: account_id.clone(),
        status: AccountStatus::Active,
        tenant_id: tenant_id.clone(),
        created_at: now,
    };
    let tenant = Tenant {
        id: tenant_id.clone(),
        status: TenantStatus::Reserved,
        namespace_binding: NamespaceBinding {
            namespace: tenant_namespace,
            database: "memory".into(),
        },
        plan_version: 1,
        schema_version: crate::http::leases::migration::CURRENT_SCHEMA_VERSION.saturating_sub(1),
        retry_stage: None,
        provisioning_lease: None,
        created_at: now,
        version: 0,
    };
    let api_key = ApiKey {
        id: cred.key_id().to_string(),
        account_id: account_id.clone(),
        name: format!("test-seed-{name}"),
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
    Ok(())
}

/// Seed a `ControlPlaneSession` for a bootstrap account so the
/// `/api/v1/account/*` middleware will accept a
/// `Cookie: __Host-memory_mcp_session=<cookie>` header. The
/// account MUST already exist (typically bootstrapped via
/// `apply_test_bootstrap`). The cookie value is opaque to the
/// server: it is hashed with `MEMORY_MCP_HTTP_SESSION_KEY` before
/// being persisted as a blind index, and the same raw value must
/// be sent back on the wire.
#[cfg(feature = "control-plane")]
async fn seed_session_one(
    state: &Arc<HttpState>,
    name: &str,
    cookie_value: &str,
) -> Result<(), MemoryError> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(MemoryError::ConfigInvalid(
            "test seed session account name must be alphanumeric/underscore".into(),
        ));
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(name.as_bytes());
    let suffix = hex::encode(&digest[..8]);
    let account_id = format!("acct_test_{suffix}");
    let store = state.registry.store_clone();
    let account = store
        .find_account_by_id(&account_id)
        .await?
        .ok_or_else(|| {
            MemoryError::ConfigInvalid(format!(
                "test seed session: bootstrap account {name} ({account_id}) not found; \
                 ensure MEMORY_MCP_HTTP_TEST_BOOTSTRAP runs first"
            ))
        })?;
    let session =
        crate::control::session::ControlPlaneSession::new(&account, cookie_value, &state.config)?;
    // The InMemoryStore refuses to overwrite an existing cookie hash
    // or session id. Restart-safe: if we have already seeded this
    // exact session in a prior run, do nothing.
    if store.find_session(&session.cookie_hash).await?.is_some() {
        return Ok(());
    }
    store.store_session(&session).await?;
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
