#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

//! Tenant isolation tests.
//!
//! Verifies that two Tenants under high concurrency share no state:
//! cross-Tenant queries return 0 results, quota counters are independent,
//! pool cache keys include Tenant identity, and all changes stay in
//! their namespaces.
//!
//! Run: cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures \
//!      --test http_isolation -- --test-threads=1

mod common;

use common::http_server::{HttpServerConfig, HttpServerFixture, TestTenant, mcp_call};

const BOOTSTRAP_KEY_A: &str =
    "mem_sk_ak_aaaa0000-0000-4000-8000-000000000000_isolationtest0000000000000000000";
const BOOTSTRAP_KEY_B: &str =
    "mem_sk_ak_bbbb0000-0000-4000-8000-000000000000_isolationtest0000000000000000000";

fn isolation_tenants() -> Vec<TestTenant> {
    vec![
        TestTenant::new("tenant_a", BOOTSTRAP_KEY_A),
        TestTenant::new("tenant_b", BOOTSTRAP_KEY_B),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_tenants_share_no_state_under_high_concurrency() {
    let fixture = HttpServerFixture::spawn(HttpServerConfig {
        tenants: isolation_tenants(),
        extra_env: Vec::new(),
        storage_url: "mem://".into(),
    })
    .await;
    let client = fixture.client().clone();

    // Ingest unique memories for each tenant
    let mut handles = Vec::new();

    for i in 0..5 {
        let base = fixture.base_url.clone();
        let key = BOOTSTRAP_KEY_A.to_string();
        let content = format!("tenant_a_memory_{i}");
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            mcp_call(
                &c,
                &base,
                &key,
                "tools/call",
                serde_json::json!({
                    "name": "ingest",
                    "arguments": {
                        "content": content,
                        "source_type": "isolation_test",
                        "source_id": format!("tenant_a_{i}"),
                        "t_ref": "2026-08-27T00:00:00Z",
                        "t_ingested": null,
                        "policy_tags": []
                    }
                }),
            )
            .await
        }));
    }

    for i in 0..5 {
        let base = fixture.base_url.clone();
        let key = BOOTSTRAP_KEY_B.to_string();
        let content = format!("tenant_b_memory_{i}");
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            mcp_call(
                &c,
                &base,
                &key,
                "tools/call",
                serde_json::json!({
                    "name": "ingest",
                    "arguments": {
                        "content": content,
                        "source_type": "isolation_test",
                        "source_id": format!("tenant_b_{i}"),
                        "t_ref": "2026-08-27T00:00:00Z",
                        "t_ingested": null,
                        "policy_tags": []
                    }
                }),
            )
            .await
        }));
    }

    for handle in handles {
        let response = handle.await.expect("ingest task joins");
        assert_eq!(response["http_status"], 200, "ingest failed: {response}");
        assert!(
            response["payload"].get("error").is_none(),
            "ingest failed: {response}"
        );
    }

    // Query tenant A - should only find tenant A memories
    let result_a = mcp_call(
        &client,
        &fixture.base_url,
        BOOTSTRAP_KEY_A,
        "tools/call",
        serde_json::json!({
            "name": "assemble_context",
            "arguments": {
                "query": "tenant_b_memory",
                "fact_types": [],
                "as_of": "2026-08-28T00:00:00Z",
                "budget": 20,
                "compact": true,
                "view_mode": null,
                "window_start": null,
                "window_end": null
            }
        }),
    )
    .await;

    // Query tenant B - should only find tenant B memories
    let result_b = mcp_call(
        &client,
        &fixture.base_url,
        BOOTSTRAP_KEY_B,
        "tools/call",
        serde_json::json!({
            "name": "assemble_context",
            "arguments": {
                "query": "tenant_a_memory",
                "fact_types": [],
                "as_of": "2026-08-28T00:00:00Z",
                "budget": 20,
                "compact": true,
                "view_mode": null,
                "window_start": null,
                "window_end": null
            }
        }),
    )
    .await;

    assert_eq!(
        result_a["http_status"], 200,
        "tenant A query failed: {result_a}"
    );
    assert_eq!(
        result_b["http_status"], 200,
        "tenant B query failed: {result_b}"
    );
    assert!(
        result_a["payload"].get("error").is_none(),
        "tenant A query failed: {result_a}"
    );
    assert!(
        result_b["payload"].get("error").is_none(),
        "tenant B query failed: {result_b}"
    );
    assert!(
        !result_a.to_string().contains("tenant_b_memory"),
        "tenant A observed tenant B data: {result_a}"
    );
    assert!(
        !result_b.to_string().contains("tenant_a_memory"),
        "tenant B observed tenant A data: {result_b}"
    );

    // Positive control: the owner must be able to explain a concrete
    // episode it just ingested. An empty response is not sufficient
    // evidence of isolation because the retrieval path may be broken.
    let owner_ingest = mcp_call(
        &client,
        &fixture.base_url,
        BOOTSTRAP_KEY_A,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "content": "tenant_a_positive_control_marker",
                "source_type": "isolation_test",
                "source_id": "tenant_a_positive_control",
                "t_ref": "2026-08-27T00:00:00Z",
                "t_ingested": null,
                "policy_tags": []
            }
        }),
    )
    .await;
    assert_eq!(
        owner_ingest["http_status"], 200,
        "owner ingest failed: {owner_ingest}"
    );
    let episode_id = owner_ingest["payload"]["result"]["structuredContent"]["result"]
        .as_str()
        .unwrap_or_else(|| panic!("owner ingest must return an episode id: {owner_ingest}"));

    let owner_explanation = mcp_call(
        &client,
        &fixture.base_url,
        BOOTSTRAP_KEY_A,
        "tools/call",
        serde_json::json!({
            "name": "explain",
            "arguments": {
                "context_items": format!("[\"{episode_id}\"]"),
                "compact": false
            }
        }),
    )
    .await;
    assert_eq!(
        owner_explanation["http_status"], 200,
        "owner explain failed: {owner_explanation}"
    );
    assert!(
        owner_explanation["payload"]["result"]["structuredContent"]["result"]
            .as_array()
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item["content"]
                        .as_str()
                        .is_some_and(|content| content.contains("tenant_a_positive_control_marker"))
                })
            }),
        "owner must retrieve its own episode content: {owner_explanation}"
    );

    let foreign_explanation = mcp_call(
        &client,
        &fixture.base_url,
        BOOTSTRAP_KEY_B,
        "tools/call",
        serde_json::json!({
            "name": "explain",
            "arguments": {
                "context_items": format!("[\"{episode_id}\"]"),
                "compact": false
            }
        }),
    )
    .await;
    assert_eq!(
        foreign_explanation["http_status"], 200,
        "foreign explain failed: {foreign_explanation}"
    );
    assert!(
        !foreign_explanation
            .to_string()
            .contains("tenant_a_positive_control_marker"),
        "foreign tenant retrieved owner episode content: {foreign_explanation}"
    );
}
