#![cfg(feature = "streamable-http")]

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use memory_mcp::tenancy::api::{
    RuntimeFactoryError, RuntimeLease, Tenancy, TenantRuntimeFactory, TenantRuntimeSpec,
};

#[derive(Debug, PartialEq, Eq)]
struct FakeRuntime {
    tenant_id: String,
    namespace: String,
}

#[derive(Default)]
struct RecordingFactory {
    activations: Mutex<Vec<(String, String, String)>>,
}

#[async_trait::async_trait]
impl TenantRuntimeFactory for RecordingFactory {
    type Runtime = FakeRuntime;

    async fn activate(
        &self,
        spec: TenantRuntimeSpec,
    ) -> Result<Self::Runtime, RuntimeFactoryError> {
        self.activations.lock().expect("activations lock").push((
            spec.tenant_id.clone(),
            spec.namespace.clone(),
            spec.database.clone(),
        ));
        Ok(FakeRuntime {
            tenant_id: spec.tenant_id,
            namespace: spec.namespace,
        })
    }
}

#[derive(Default)]
struct BlockedFactory {
    activations: AtomicUsize,
    entered: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl TenantRuntimeFactory for BlockedFactory {
    type Runtime = FakeRuntime;

    async fn activate(
        &self,
        spec: TenantRuntimeSpec,
    ) -> Result<Self::Runtime, RuntimeFactoryError> {
        self.activations.fetch_add(1, Ordering::SeqCst);
        if self.activations.load(Ordering::SeqCst) == 1 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(FakeRuntime {
            tenant_id: spec.tenant_id,
            namespace: spec.namespace,
        })
    }
}

fn spec(tenant_id: &str, namespace: &str) -> TenantRuntimeSpec {
    TenantRuntimeSpec {
        tenant_id: tenant_id.into(),
        namespace: namespace.into(),
        database: "memory".into(),
        plan_version: 1,
        schema_version: 7,
    }
}

#[tokio::test]
async fn runtime_factory_keeps_tenant_activations_isolated() {
    let factory = Arc::new(RecordingFactory::default());
    let tenancy = Tenancy::new(
        factory.clone(),
        4,
        std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(1),
    );
    let a = tenancy
        .activate(spec("ten_a", "tns_a"))
        .await
        .expect("activate A");
    let b = tenancy
        .activate(spec("ten_b", "tns_b"))
        .await
        .expect("activate B");
    assert_eq!(a.runtime().tenant_id, "ten_a");
    assert_eq!(a.runtime().namespace, "tns_a");
    assert_eq!(b.runtime().tenant_id, "ten_b");
    assert_eq!(b.runtime().namespace, "tns_b");
    drop(a);
    drop(b);

    let a_again = tenancy
        .activate(spec("ten_a", "tns_a"))
        .await
        .expect("reuse A");
    assert_eq!(a_again.runtime().namespace, "tns_a");
    let activations = factory.activations.lock().expect("activations lock");
    assert_eq!(activations.len(), 2);
    let unique_namespaces = activations
        .iter()
        .map(|(_, namespace, _)| namespace.clone())
        .collect::<HashSet<_>>();
    assert_eq!(unique_namespaces.len(), 2);

    let a_lease: RuntimeLease<FakeRuntime> = a_again;
    assert_eq!(a_lease.runtime().tenant_id, "ten_a");
}

#[tokio::test]
async fn concurrent_tenant_acquisitions_single_flight_each_binding() {
    let factory = Arc::new(RecordingFactory::default());
    let tenancy = Arc::new(Tenancy::new(
        factory.clone(),
        8,
        std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(1),
    ));
    let mut tasks = Vec::new();
    for index in 0..8 {
        let tenancy = tenancy.clone();
        tasks.push(tokio::spawn(async move {
            let id = if index % 2 == 0 { "ten_a" } else { "ten_b" };
            tenancy
                .activate(spec(id, &format!("tns_{}", id)))
                .await
                .expect("activate")
        }));
    }
    for task in tasks {
        let lease = task.await.expect("task");
        assert_eq!(
            lease.runtime().namespace,
            format!("tns_{}", lease.runtime().tenant_id)
        );
    }
    assert_eq!(
        factory.activations.lock().expect("activations lock").len(),
        2
    );
}

#[tokio::test]
async fn blocked_first_activation_single_flights_all_waiters() {
    let factory = Arc::new(BlockedFactory::default());
    let tenancy = Arc::new(Tenancy::new(
        factory.clone(),
        4,
        std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(2),
    ));
    let first = {
        let tenancy = tenancy.clone();
        tokio::spawn(async move { tenancy.activate(spec("ten_a", "tns_a")).await })
    };
    factory.entered.notified().await;
    let second = {
        let tenancy = tenancy.clone();
        tokio::spawn(async move { tenancy.activate(spec("ten_a", "tns_a")).await })
    };
    tokio::task::yield_now().await;
    assert_eq!(factory.activations.load(Ordering::SeqCst), 1);
    factory.release.notify_waiters();
    let _ = first.await.expect("first task").expect("first lease");
    let _ = second.await.expect("second task").expect("second lease");
}

#[tokio::test]
async fn idle_eviction_reactivates_same_immutable_binding() {
    let factory = Arc::new(RecordingFactory::default());
    let tenancy = Tenancy::new(
        factory.clone(),
        2,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
    );
    let lease = tenancy
        .activate(spec("ten_a", "tns_a"))
        .await
        .expect("first");
    assert_eq!(lease.runtime().namespace, "tns_a");
    drop(lease);
    assert_eq!(tenancy.evict_idle().await, 1);
    let lease = tenancy
        .activate(spec("ten_a", "tns_a"))
        .await
        .expect("second");
    assert_eq!(lease.runtime().namespace, "tns_a");
    assert_eq!(
        factory.activations.lock().expect("activations lock").len(),
        2
    );
}
