//! Durable subscription replica integration suite (Task 7 of the
//! architecture audit remediation plan).
//!
//! Black-box coverage for one tenant's durable subscription store,
//! exercised through the `SubscriptionTestDriver` exposed by
//! `http::subscriptions`. The "second replica from cursor" and
//! "missed wakeup repaired by durable polling" cases use two independent
//! `BoundDbClient` handles against the same tenant namespace so
//! the second reader can observe the first writer's durable
//! commits and prove the polling path repairs a missed wake hint. These tests
//! do not prove process-restart persistence.
//!
//! Run:
//! cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures \
//!     --test http_subscription_replica -- --test-threads=1

#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

use std::sync::Arc;

use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::subscriptions::stream::CoalescingQueue;
use memory_mcp::http::subscriptions::{SubscriptionTestDriver, ValidatedSubscriptionFilter};
use memory_mcp::platform::persistence::outbox::TenantChangeEvent;
use memory_mcp::storage::BoundDbClient;

/// Two independent `BoundDbClient` handles bound to the same
/// tenant namespace, with a `SubscriptionTestDriver` for each.
struct TwoDrivers {
    writer: SubscriptionTestDriver,
    reader: SubscriptionTestDriver,
    _registry: RegistryHandle,
}

async fn two_drivers(tenant_id: &str, namespace: &str) -> TwoDrivers {
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let client_a = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let client_b = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let writer = SubscriptionTestDriver::new(
        Arc::new(BoundDbClient::new(client_a, namespace.to_owned())),
        tenant_id.to_owned(),
    );
    let reader = SubscriptionTestDriver::new(
        Arc::new(BoundDbClient::new(client_b, namespace.to_owned())),
        tenant_id.to_owned(),
    );
    writer
        .apply_schema_for_test(namespace)
        .await
        .expect("apply schema for writer");
    TwoDrivers {
        writer,
        reader,
        _registry: registry,
    }
}

fn root_filter(tenant_id: &str, app: &str) -> ValidatedSubscriptionFilter {
    ValidatedSubscriptionFilter::for_tenant(
        tenant_id,
        &rmcp::model::SubscriptionFilter::builder()
            .resource_subscription(format!("ui://memory/apps/{app}"))
            .build(),
    )
    .expect("valid root filter")
}

fn event(sequence: u64, resource_id: &str, revision: u64) -> TenantChangeEvent {
    TenantChangeEvent {
        sequence,
        resource_id: resource_id.to_string(),
        revision,
        change_kind: "updated".to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn filter_validation_rejects_non_resource_fields() {
    // A filter with `tools_list_changed` is rejected outright;
    // the durable store only accepts resource URI lists.
    let unsupported = rmcp::model::SubscriptionFilter::builder()
        .tools_list_changed()
        .resource_subscription("ui://memory/apps/graph")
        .build();
    let err = ValidatedSubscriptionFilter::for_tenant("tenant_a", &unsupported)
        .expect_err("non-resource filter must be rejected");
    assert!(matches!(err, memory_mcp::error::MemoryError::Validation(_)));
}

#[tokio::test]
async fn sequence_is_monotonic_across_appends() {
    let h = two_drivers("tenant_seq", "ns_seq").await;
    h.writer
        .append_event_for_test(&event(1, "ui://memory/apps/graph", 1))
        .await
        .unwrap();
    h.writer
        .append_event_for_test(&event(2, "ui://memory/apps/graph", 2))
        .await
        .unwrap();
    h.writer
        .append_event_for_test(&event(3, "ui://memory/apps/inspector", 1))
        .await
        .unwrap();
    let filter = root_filter("tenant_seq", "graph");
    let batch = h.writer.next_batch(0, &filter).await.unwrap();
    assert_eq!(batch.len(), 2, "expected 2 graph events, got {batch:?}");
    assert_eq!(batch[0].sequence, 1);
    assert_eq!(batch[1].sequence, 2);
    // Advancing the cursor past the committed graph events
    // returns nothing (the only post-cursor event is for
    // `inspector`, filtered out by this test's resource filter).
    let batch = h.writer.next_batch(2, &filter).await.unwrap();
    assert!(batch.is_empty(), "graph filter must not return inspector");
}

#[tokio::test]
async fn bounded_coalescing_replaces_with_higher_revision_only() {
    let mut q = CoalescingQueue::new(2);
    assert_eq!(
        q.push(event(1, "ui://memory/apps/graph", 1)).unwrap(),
        memory_mcp::http::subscriptions::stream::QueuePush::Enqueued,
    );
    // A higher revision on the same resource replaces the prior entry.
    assert_eq!(
        q.push(event(2, "ui://memory/apps/graph", 2)).unwrap(),
        memory_mcp::http::subscriptions::stream::QueuePush::Coalesced,
    );
    // An older revision does not overwrite.
    assert_eq!(
        q.push(event(3, "ui://memory/apps/graph", 1)).unwrap(),
        memory_mcp::http::subscriptions::stream::QueuePush::Coalesced,
    );
    assert_eq!(q.len(), 1);
    // The single stored entry is the highest-revision one.
    let popped = q.pop_front().expect("single coalesced entry");
    assert_eq!(popped.revision, 2);
    // A different resource enqueues separately. The queue
    // already had one entry which we just popped, so after this
    // push it holds one entry again.
    assert_eq!(
        q.push(event(4, "ui://memory/apps/inspector", 1)).unwrap(),
        memory_mcp::http::subscriptions::stream::QueuePush::Enqueued,
    );
    assert_eq!(q.len(), 1);
    // A second distinct resource fills the bounded capacity.
    assert_eq!(
        q.push(event(5, "ui://memory/apps/lifecycle", 1)).unwrap(),
        memory_mcp::http::subscriptions::stream::QueuePush::Enqueued,
    );
    assert_eq!(q.len(), 2);
    // A third distinct resource overflows the bounded capacity.
    use memory_mcp::http::subscriptions::stream::QueueError;
    assert_eq!(
        q.push(event(6, "ui://memory/apps/graph", 1)).unwrap_err(),
        QueueError::Full,
    );
}

#[tokio::test]
async fn missed_wakeup_is_repaired_by_durable_polling() {
    // A "missed wakeup" simulates the case where the writer
    // commits an event without the replica receiving a wake
    // hint. The replica's polling path (`next_batch` after the
    // committed sequence) must return the event.
    let h = two_drivers("tenant_polling", "ns_polling").await;
    let filter = root_filter("tenant_polling", "graph");
    // Replica polls first: nothing to read.
    let pre = h.reader.next_batch(0, &filter).await.unwrap();
    assert!(pre.is_empty(), "no commits yet");
    // Writer commits an event. The replica did not receive a
    // wake hint (e.g. its listener was disconnected); the next
    // poll must still observe it.
    h.writer
        .append_event_for_test(&event(1, "ui://memory/apps/graph", 1))
        .await
        .unwrap();
    let observed = h.reader.next_batch(0, &filter).await.unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].sequence, 1);
}

#[tokio::test]
async fn second_replica_picks_up_from_committed_cursor() {
    // A second replica that opens after the writer commits must
    // see the same committed events when it polls from
    // after_sequence = 0.
    let h = two_drivers("tenant_restart", "ns_restart").await;
    let filter = root_filter("tenant_restart", "graph");
    for seq in 1..=3 {
        h.writer
            .append_event_for_test(&event(seq, "ui://memory/apps/graph", seq))
            .await
            .unwrap();
    }
    // The "second replica" is `h.reader` (same namespace,
    // different connection). It starts at sequence 0 and
    // observes the full committed log.
    let events = h.reader.next_batch(0, &filter).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[2].sequence, 3);
    // Advancing the cursor past committed events returns nothing.
    let drained = h.reader.next_batch(3, &filter).await.unwrap();
    assert!(drained.is_empty());
}

#[tokio::test]
async fn authorization_expiry_rejects_live_session_filter() {
    // A filter pinning a concrete app session must be rejected
    // until the session is provisioned in the tenant's
    // `app_session` table. The integration path mirrors the
    // existing module test, but exercises the
    // driver-level `validate_filter` to confirm the durable
    // store carries the same authorization check.
    let h = two_drivers("tenant_authz", "ns_authz").await;
    let filter = ValidatedSubscriptionFilter::for_tenant(
        "tenant_authz",
        &rmcp::model::SubscriptionFilter::builder()
            .resource_subscription("ui://memory/app/graph/session-1")
            .build(),
    )
    .expect("valid session URI");
    let err = h
        .writer
        .validate_filter(&filter)
        .await
        .expect_err("concrete session without a live row must be rejected");
    assert!(matches!(err, memory_mcp::error::MemoryError::Auth(_)));
}

#[tokio::test]
async fn cross_tenant_subscription_is_rejected() {
    // A store bound to tenant_a must not serve a filter that
    // names tenant_b, even when the resource URIs are valid.
    let h = two_drivers("tenant_a", "ns_x").await;
    let wrong = root_filter("tenant_b", "graph");
    let err =
        h.writer.next_batch(0, &wrong).await.expect_err(
            "filter for another tenant must be rejected before any data leaves the store",
        );
    assert!(matches!(err, memory_mcp::error::MemoryError::Auth(_)));
}

#[tokio::test]
async fn subscription_eviction_releases_unrelated_tenant_runtime_dependencies() {
    use std::time::Duration;

    use async_trait::async_trait;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use memory_mcp::error::MemoryError;
    use memory_mcp::http::config::HttpConfig;
    use memory_mcp::http::principal::AuthenticatedPrincipal;
    use memory_mcp::http::principal::auth::{Authenticator, RateLimiter};
    use memory_mcp::http::principal::cache::PrincipalCache;
    use memory_mcp::http::registry::models::{
        Account, AccountStatus, ApiKey, ApiKeyStatus, KeyedVerifier,
    };
    use memory_mcp::http::registry::storage::{AccountStore, ApiKeyStore, InMemoryStore};
    use memory_mcp::http::subscriptions::SubscriptionStore;
    use memory_mcp::http::test_state::HttpStateTestBuilder;
    use memory_mcp::tenancy::api::{TenantLifecycleStatus, TenantRuntimeSpec};
    use serde_json::{Value, json};
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    struct DropProbeSubscriptionStore {
        started: Arc<Notify>,
        dropped: Arc<Notify>,
    }

    impl Drop for DropProbeSubscriptionStore {
        fn drop(&mut self) {
            self.dropped.notify_one();
        }
    }

    #[async_trait]
    impl SubscriptionStore for DropProbeSubscriptionStore {
        async fn current_sequence(&self) -> Result<u64, MemoryError> {
            self.started.notify_one();
            Ok(0)
        }

        async fn next_batch(
            &self,
            _after_sequence: u64,
            _filter: &ValidatedSubscriptionFilter,
        ) -> Result<Vec<TenantChangeEvent>, MemoryError> {
            Ok(Vec::new())
        }
    }

    let mut http_config = HttpConfig::default_for_test();
    http_config.runtime_idle_ttl = Duration::ZERO;
    let state = HttpStateTestBuilder::new()
        .await
        .with_config(http_config)
        .build()
        .await
        .expect("HTTP test state builds");
    let runtime_spec = TenantRuntimeSpec {
        tenant_id: "tenant_subscription".to_string(),
        namespace: "ns_subscription".to_string(),
        database: "memory".to_string(),
        plan_version: 1,
        schema_version: 7,
        status: TenantLifecycleStatus::Ready,
    };
    let operation = state
        .pool
        .acquire_spec_with_limit(&runtime_spec, 4)
        .await
        .expect("tenant runtime activates");
    let runtime_weak = Arc::downgrade(operation.runtime());
    let service_weak = Arc::downgrade(&operation.runtime().mcp_service.service());

    let registry = Arc::new(InMemoryStore::default());
    let account = Account {
        id: "account_subscription".to_string(),
        status: AccountStatus::Active,
        tenant_id: "tenant_subscription".to_string(),
        created_at: chrono::Utc::now(),
        display_name: None,
    };
    let api_key = ApiKey {
        id: "key_subscription".to_string(),
        account_id: account.id.clone(),
        name: "ownership test".to_string(),
        verifier: KeyedVerifier([0; 32]),
        status: ApiKeyStatus::Active,
        created_at: chrono::Utc::now(),
        expires_at: None,
        last_used_at: None,
        version: 0,
    };
    AccountStore::write_account(registry.as_ref(), &account)
        .await
        .expect("seed authenticated account");
    ApiKeyStore::write_api_key(registry.as_ref(), &api_key)
        .await
        .expect("seed active subscription key");
    let authenticator = Arc::new(Authenticator::new(
        registry.clone(),
        registry,
        Arc::new(PrincipalCache::new(8)),
        Vec::new(),
        Arc::new(RateLimiter::new(8, Duration::from_secs(1), 32)),
    ));
    let principal = AuthenticatedPrincipal::ApiKey {
        account: Arc::new(account),
        key_id: api_key.id,
    };

    let store_started = Arc::new(Notify::new());
    let store_dropped = Arc::new(Notify::new());
    let subscription_store = Arc::new(DropProbeSubscriptionStore {
        started: store_started.clone(),
        dropped: store_dropped.clone(),
    });
    let store_weak = Arc::downgrade(&subscription_store);
    let shutdown = CancellationToken::new();
    let handler = operation
        .runtime()
        .mcp_service
        .clone()
        .with_tenant_id("tenant_subscription")
        .with_durable_subscriptions(subscription_store.clone())
        .with_subscription_authorization(principal, authenticator.clone())
        .with_subscription_limits(4, Duration::from_secs(60));
    drop(operation);
    let filter = rmcp::model::SubscriptionFilter::builder()
        .resource_subscription("ui://memory/apps/graph")
        .build();
    let mut params =
        serde_json::to_value(rmcp::model::SubscriptionsListenRequestParams::new(filter))
            .expect("serialize listen parameters");
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "ownership-test", "version": "0.0.0"},
        "io.modelcontextprotocol/clientCapabilities": {}
    });

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "subscriptions/listen")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 31,
                "method": "subscriptions/listen",
                "params": params
            })
            .to_string(),
        ))
        .expect("subscription request builds");
    let response = memory_mcp::http::transport::forward_subscription_for_test(
        handler,
        request,
        HttpConfig::default_for_test(),
        shutdown.clone(),
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let mut body = response.into_body();
    let first_frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
        .await
        .expect("stream acknowledgment arrives")
        .expect("the response starts an SSE stream")
        .expect("SSE frame is valid");
    let first_data = first_frame
        .into_data()
        .expect("the first SSE response is a data frame");
    let ack = std::str::from_utf8(&first_data).expect("ack frame is UTF-8");
    let ack_json: Value = serde_json::from_str(
        ack.strip_prefix("data: ")
            .expect("ack frame uses SSE data framing")
            .trim(),
    )
    .expect("ack frame contains JSON");
    assert_eq!(
        ack_json["params"]["notifications"]["resourceSubscriptions"][0],
        "ui://memory/apps/graph"
    );
    tokio::time::timeout(Duration::from_secs(2), store_started.notified())
        .await
        .expect("the live subscription enters its durable polling loop");

    drop(subscription_store);
    drop(authenticator);

    assert_eq!(state.pool.evict_idle().await, 1);
    assert!(
        runtime_weak.upgrade().is_none(),
        "idle eviction releases the prior tenant runtime"
    );
    assert!(
        service_weak.upgrade().is_none(),
        "an active stream must not retain the evicted tenant service generation"
    );
    let reactivated = state
        .pool
        .acquire_spec_with_limit(&runtime_spec, 4)
        .await
        .expect("same tenant runtime reactivates while subscription remains active");
    let reactivated_service_weak = Arc::downgrade(&reactivated.runtime().mcp_service.service());
    assert!(
        !service_weak.ptr_eq(&reactivated_service_weak),
        "reactivation uses a distinct tenant service generation"
    );
    drop(reactivated);
    assert!(
        store_weak.upgrade().is_some(),
        "the stream keeps its subscription store alive while polling"
    );
    assert!(
        service_weak.upgrade().is_none(),
        "the active subscription owns its narrow dependencies, not MemoryService"
    );

    assert_eq!(state.pool.evict_idle().await, 1);
    assert!(
        reactivated_service_weak.upgrade().is_none(),
        "a later idle generation is also released while the old stream is active"
    );
    let reactivated_again = state
        .pool
        .acquire_spec_with_limit(&runtime_spec, 4)
        .await
        .expect("the tenant can reactivate repeatedly during an older stream");
    assert!(
        !reactivated_service_weak.ptr_eq(&Arc::downgrade(
            &reactivated_again.runtime().mcp_service.service()
        )),
        "each reactivation creates a new tenant service generation"
    );
    drop(reactivated_again);
    assert_eq!(state.pool.evict_idle().await, 1);
    assert!(
        reactivated_service_weak.upgrade().is_none(),
        "repeated eviction does not leave duplicate service generations alive"
    );

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), store_dropped.notified())
        .await
        .expect("server shutdown releases the subscription store");
    assert!(
        store_weak.upgrade().is_none(),
        "the subscription store is released when its stream ends"
    );
    drop(body);
}
