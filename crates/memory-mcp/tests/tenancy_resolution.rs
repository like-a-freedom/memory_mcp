#![cfg(feature = "streamable-http")]

use std::sync::Mutex;

use memory_mcp::MemoryError;
use memory_mcp::tenancy::api::{
    ResolveTenantPort, TenantResolution, TenantResolutionStatus, TenantRuntimeSpec,
    resolve_tenant_runtime,
};

#[derive(Default)]
struct RecordingResolver {
    result: Mutex<Option<TenantResolution>>,
}

#[async_trait::async_trait]
impl ResolveTenantPort for RecordingResolver {
    async fn resolve_tenant(&self, _account_id: &str) -> Result<TenantResolution, MemoryError> {
        Ok(self
            .result
            .lock()
            .expect("result lock")
            .clone()
            .expect("resolution result"))
    }
}

fn ready() -> TenantResolution {
    TenantResolution::Ready(TenantRuntimeSpec {
        tenant_id: "ten_1".into(),
        namespace: "tns_1".into(),
        database: "memory".into(),
        plan_version: 1,
        schema_version: 7,
        status: memory_mcp::tenancy::api::TenantLifecycleStatus::Ready,
    })
}

#[tokio::test]
async fn tenant_resolution_returns_the_server_owned_runtime_binding() {
    let port = RecordingResolver::default();
    *port.result.lock().expect("result lock") = Some(ready());

    let spec = resolve_tenant_runtime(&port, "acct_1")
        .await
        .expect("resolve ready tenant");

    assert_eq!(spec.tenant_id, "ten_1");
    assert_eq!(spec.namespace, "tns_1");
    assert_eq!(spec.database, "memory");
}

#[tokio::test]
async fn tenant_resolution_maps_missing_tenant_to_not_found() {
    let port = RecordingResolver::default();
    *port.result.lock().expect("result lock") = Some(TenantResolution::NotFound);

    let error = resolve_tenant_runtime(&port, "acct_1")
        .await
        .expect_err("missing tenant is refused");

    assert!(error.to_string().contains("not found"));
}

#[tokio::test]
async fn tenant_resolution_maps_suspended_tenant_to_auth_refusal() {
    let port = RecordingResolver::default();
    *port.result.lock().expect("result lock") = Some(TenantResolution::Suspended);

    let error = resolve_tenant_runtime(&port, "acct_1")
        .await
        .expect_err("suspended tenant is refused");

    assert!(error.to_string().contains("suspended"));
}

#[tokio::test]
async fn tenant_resolution_maps_failed_tenant_to_unavailable() {
    let port = RecordingResolver::default();
    *port.result.lock().expect("result lock") = Some(TenantResolution::Failed("ten_1".into()));

    let error = resolve_tenant_runtime(&port, "acct_1")
        .await
        .expect_err("failed tenant is refused");

    assert!(error.to_string().contains("failed"));
}

#[tokio::test]
async fn tenant_resolution_maps_provisioning_tenant_to_unavailable() {
    let port = RecordingResolver::default();
    *port.result.lock().expect("result lock") = Some(TenantResolution::Provisioning(
        TenantResolutionStatus::Migrating,
    ));

    let error = resolve_tenant_runtime(&port, "acct_1")
        .await
        .expect_err("provisioning tenant is refused");

    assert!(error.to_string().contains("provisioning"));
}
