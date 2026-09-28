#![cfg(feature = "streamable-http")]

use std::sync::Mutex;

use memory_mcp::MemoryError;
use memory_mcp::tenancy::api::{
    ResolveTenantPort, TenantBinding, TenantLifecycleStatus, TenantResolution,
    TenantResolutionStatus, TenantRuntimeSpec, resolve_tenant_runtime,
};

#[derive(Default)]
struct RecordingResolver {
    result: Mutex<Option<TenantResolution>>,
    binding: Mutex<Option<TenantBinding>>,
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

    async fn resolve_tenant_for_maintenance(
        &self,
        _tenant_id: &str,
    ) -> Result<TenantBinding, MemoryError> {
        Ok(self
            .binding
            .lock()
            .expect("binding lock")
            .clone()
            .expect("binding"))
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

/// The maintenance binding path is not a way around the request-path refusals.
///
/// `resolve_tenant_for_maintenance` exists so a worker can reach a tenant that
/// is being deleted. If it could also hand back a spec that `resolve_tenant_runtime`
/// accepts, then any caller holding a resolver could reach a suspended or failed
/// tenant's namespace by asking for it the other way — and the whole point of
/// refusing a non-`Ready` tenant at request time is that the namespace is not
/// served.
#[tokio::test]
async fn a_maintenance_binding_never_reaches_the_request_path() {
    let port = RecordingResolver::default();
    let binding = TenantBinding {
        spec: TenantRuntimeSpec {
            tenant_id: "ten_1".into(),
            namespace: "tns_1".into(),
            database: "memory".into(),
            plan_version: 1,
            schema_version: 7,
            status: TenantLifecycleStatus::Deleting,
        },
        status: TenantLifecycleStatus::Deleting,
    };
    *port.binding.lock().expect("binding lock") = Some(binding);

    let resolved = port
        .resolve_tenant_for_maintenance("ten_1")
        .await
        .expect("a deleting tenant is bindable for maintenance");

    assert_eq!(resolved.spec.tenant_id, "ten_1");
    assert_eq!(resolved.status, TenantLifecycleStatus::Deleting);

    // The request path still refuses it: the resolver's own request-path
    // result is unchanged by anything the maintenance path did.
    *port.result.lock().expect("result lock") = Some(TenantResolution::Suspended);
    let error = resolve_tenant_runtime(&port, "acct_1")
        .await
        .expect_err("a deleting tenant must not serve a request");
    assert!(error.to_string().contains("suspended"));
}
