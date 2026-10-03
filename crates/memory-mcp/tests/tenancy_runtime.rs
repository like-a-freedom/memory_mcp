#![cfg(feature = "streamable-http")]

//! Trusted tenant resolution and the runtime factory port.
//!
//! The activation lifecycle that used to be asserted here — single-flight,
//! blocked-factory waiters, idle eviction re-activating a binding — moved to
//! `http::runtime::pool`'s own tests when the pool became the single owner of
//! activation and residency (ADR-0072). Asserting that behaviour against a
//! second cache is what let a cancellation bug hide: the cache that was not
//! being tested still held the producer.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use memory_mcp::tenancy::api::{
    ResolveTenantPort, RuntimeFactoryError, TenantBinding, TenantLifecycleStatus, TenantResolution,
    TenantRuntimeFactory, TenantRuntimeSpec, resolve_tenant_runtime,
};

fn spec(tenant_id: &str, namespace: &str) -> TenantRuntimeSpec {
    TenantRuntimeSpec {
        tenant_id: tenant_id.into(),
        namespace: namespace.into(),
        database: "memory".into(),
        plan_version: 1,
        schema_version: 7,
        status: TenantLifecycleStatus::Ready,
    }
}

#[test]
fn identity_is_the_tenant_and_its_storage() {
    let identity = spec("ten_a", "tns_a").identity();

    assert_eq!(identity.tenant_id, "ten_a");
    assert_eq!(identity.namespace, "tns_a");
    assert_eq!(identity.database, "memory");
}

/// Plan, schema and status describe the runtime revision and the caller's
/// permission, not which storage the runtime is bound to. Treating any of them
/// as identity would refuse a legitimate change instead of replacing a runtime
/// the change made stale, and would cache an authorization decision.
#[test]
fn identity_ignores_plan_schema_and_status() {
    let mut changed = spec("ten_a", "tns_a");
    changed.plan_version = 9;
    changed.schema_version = 12;
    changed.status = TenantLifecycleStatus::Deleting;

    assert_eq!(changed.identity(), spec("ten_a", "tns_a").identity());
}

#[test]
fn identity_separates_tenants_and_their_namespaces() {
    let base = spec("ten_a", "tns_a").identity();

    assert_ne!(base, spec("ten_b", "tns_a").identity());
    assert_ne!(base, spec("ten_a", "tns_b").identity());
    let mut other_database = spec("ten_a", "tns_a");
    other_database.database = "other".into();
    assert_ne!(base, other_database.identity());
}

/// A runtime is assembled only through the declared port. The port is what lets
/// the acquisition path be driven by a controlled factory in tests, so a
/// production adapter and a test adapter are the two implementations that
/// justify the seam.
#[tokio::test]
async fn the_factory_port_receives_the_spec_it_is_asked_to_build() {
    #[derive(Default)]
    struct RecordingFactory {
        seen: std::sync::Mutex<Vec<(String, String, u32)>>,
    }

    #[async_trait::async_trait]
    impl TenantRuntimeFactory for RecordingFactory {
        type Runtime = String;

        async fn activate(
            &self,
            spec: TenantRuntimeSpec,
        ) -> Result<Self::Runtime, RuntimeFactoryError> {
            self.seen.lock().expect("seen lock").push((
                spec.tenant_id.clone(),
                spec.namespace.clone(),
                spec.schema_version,
            ));
            Ok(format!("{}@{}", spec.tenant_id, spec.namespace))
        }
    }

    let factory = Arc::new(RecordingFactory::default());
    let runtime = factory
        .activate(spec("ten_a", "tns_a"))
        .await
        .expect("activate");

    assert_eq!(runtime, "ten_a@tns_a");
    assert_eq!(
        factory.seen.lock().expect("seen lock").as_slice(),
        [("ten_a".to_string(), "tns_a".to_string(), 7)]
    );
}

/// The request path refuses every resolution but `Ready`. A resolver that
/// answers `Suspended` or a provisioning status must not produce a runtime.
#[tokio::test]
async fn resolve_tenant_runtime_returns_a_spec_only_for_a_ready_tenant() {
    struct FixedResolver {
        answer: TenantResolution,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ResolveTenantPort for FixedResolver {
        async fn resolve_tenant(
            &self,
            _account_id: &str,
        ) -> Result<TenantResolution, memory_mcp::MemoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.answer.clone())
        }

        async fn resolve_tenant_for_maintenance(
            &self,
            _tenant_id: &str,
        ) -> Result<TenantBinding, memory_mcp::MemoryError> {
            Ok(TenantBinding {
                spec: spec("ten_a", "tns_a"),
                status: TenantLifecycleStatus::Deleting,
            })
        }
    }

    let ready = FixedResolver {
        answer: TenantResolution::Ready(spec("ten_a", "tns_a")),
        calls: AtomicUsize::new(0),
    };
    let resolved = resolve_tenant_runtime(&ready, "acct")
        .await
        .expect("a ready tenant resolves");
    assert_eq!(resolved.tenant_id, "ten_a");

    for refusal in [
        TenantResolution::NotFound,
        TenantResolution::Suspended,
        TenantResolution::Failed("ten_a".into()),
        TenantResolution::Provisioning(memory_mcp::tenancy::api::TenantResolutionStatus::Migrating),
    ] {
        let resolver = FixedResolver {
            answer: refusal,
            calls: AtomicUsize::new(0),
        };
        assert!(
            resolve_tenant_runtime(&resolver, "acct").await.is_err(),
            "a non-ready tenant must not resolve to a runtime spec"
        );
    }
}
