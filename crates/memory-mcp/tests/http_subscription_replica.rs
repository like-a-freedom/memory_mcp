//! Durable subscription replica integration suite (Task 7 of the
//! architecture audit remediation plan).
//!
//! Black-box coverage for one tenant's durable subscription store,
//! exercised through the `SubscriptionTestDriver` exposed by
//! `http::subscriptions`. The "restart from cursor" and "missed
//! wakeup repaired by durable polling" cases use two independent
//! `BoundDbClient` handles against the same tenant namespace so
//! the second reader can observe the first writer's durable
//! commits and prove the polling path repairs a missed wake hint.
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
use memory_mcp::http::subscriptions::outbox::TenantChangeEvent;
use memory_mcp::http::subscriptions::stream::CoalescingQueue;
use memory_mcp::http::subscriptions::{SubscriptionTestDriver, ValidatedSubscriptionFilter};
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
async fn restart_picks_up_from_committed_cursor() {
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
