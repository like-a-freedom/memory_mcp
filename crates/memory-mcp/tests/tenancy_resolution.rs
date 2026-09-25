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
            .expect("result"))
    }
}

fn ready() -> TenantResolution {
    TenantResolution::Ready(TenantRuntimeSpec {
        tenant_id: "ten_1".into(),
        namespace: "tns_1".into(),
        database: "memory".into(),
        plan_version: 1,
        schema_version: 7,
    })
}

#[tokio::test]
async fn tenant_resolution_returns_only_server_owned_runtime_spec() {
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
async fn tenant_resolution_preserves_non_ready_outcomes() {
    let cases = [
        (TenantResolution::NotFound, "not found"),
        (TenantResolution::Suspended, "suspended"),
        (TenantResolution::Failed("ten_1".into()), "failed"),
        (
            TenantResolution::Provisioning(TenantResolutionStatus::Migrating),
            "provisioning",
        ),
    ];
    for (resolution, expected) in cases {
        let port = RecordingResolver::default();
        *port.result.lock().expect("result lock") = Some(resolution);
        let error = resolve_tenant_runtime(&port, "acct_1")
            .await
            .expect_err("non-ready outcome");
        assert!(error.to_string().contains(expected));
    }
}
