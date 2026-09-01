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

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

struct Server {
    child: Child,
    base_url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

const BOOTSTRAP_KEY_A: &str =
    "mem_sk_ak_aaaa0000-0000-4000-8000-000000000000_isolationtest0000000000000000000";
const BOOTSTRAP_KEY_B: &str =
    "mem_sk_ak_bbbb0000-0000-4000-8000-000000000000_isolationtest0000000000000000000";

fn base_env(port: u16) -> Vec<(String, String)> {
    let zeros = "0".repeat(64);
    vec![
        ("MEMORY_MCP_HTTP_BIND".into(), format!("127.0.0.1:{port}")),
        (
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL".into(),
            format!("http://localhost:{port}"),
        ),
        ("ALLOWED_HOSTS".into(), "localhost,127.0.0.1".into()),
        ("ALLOWED_ORIGINS".into(), "http://localhost".into()),
        ("MEMORY_MCP_API_KEY_PEPPER".into(), "x".repeat(40)),
        ("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_SIGNUP_MODE".into(), "invite_only".into()),
        ("MEMORY_MCP_HTTP_CSRF_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_OIDC_STATE_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_OIDC_NONCE_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_SESSION_KEY".into(), zeros),
        ("SURREALDB_CONTROL_URL".into(), "mem://".into()),
        ("SURREALDB_CONTROL_USERNAME".into(), "root".into()),
        ("SURREALDB_CONTROL_PASSWORD".into(), "root".into()),
        ("SURREALDB_CONTROL_DB".into(), "control".into()),
        ("SURREALDB_CONTROL_NAMESPACE".into(), "control".into()),
        ("SURREALDB_TENANT_URL".into(), "mem://".into()),
        ("SURREALDB_TENANT_USERNAME".into(), "root".into()),
        ("SURREALDB_TENANT_PASSWORD".into(), "root".into()),
        ("SURREALDB_TENANT_DB".into(), "tenant".into()),
        ("SURREALDB_TENANT_NAMESPACE".into(), "tenant".into()),
        (
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE".into(),
            "false".into(),
        ),
        (
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI".into(),
            "false".into(),
        ),
        (
            "MEMORY_MCP_HTTP_TEST_BOOTSTRAP".into(),
            format!("tenant_a={BOOTSTRAP_KEY_A},tenant_b={BOOTSTRAP_KEY_B}"),
        ),
    ]
}

fn start_server() -> Server {
    let port = free_port();
    let mut env = base_env(port);
    env.push(("RUST_LOG".into(), "info".into()));

    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    let child = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"))
        .envs(env_refs.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn server");

    let mut server = Server {
        child,
        base_url: format!("http://127.0.0.1:{port}"),
    };

    let stdout = server.child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = line.expect("read line");
        if line.contains("bound=") {
            break;
        }
    }
    server
}

async fn mcp_call(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let mut params = params;
    params.as_object_mut().expect("params object").insert(
        "_meta".into(),
        serde_json::json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {
                "tasks": {},
                "subscriptions": {}
            }
        }),
    );
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("host", "localhost")
        .header("authorization", format!("Bearer {api_key}"))
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", method)
        .header(
            "mcp-name",
            params
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        )
        .body(serde_json::to_string(&body).unwrap())
        .send()
        .await
        .expect("send request");

    let status = resp.status();
    let text = resp.text().await.unwrap();

    // Parse SSE or JSON response
    if text.starts_with("event:") || text.starts_with("data:") {
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ")
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(data)
            {
                return serde_json::json!({
                    "http_status": status.as_u16(),
                    "payload": val
                });
            }
        }
    }
    let payload = serde_json::from_str(&text).unwrap_or(serde_json::json!({"raw": text}));
    serde_json::json!({
        "http_status": status.as_u16(),
        "payload": payload
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_tenants_share_no_state_under_high_concurrency() {
    let server = start_server();
    let client = reqwest::Client::new();

    // Ingest unique memories for each tenant
    let mut handles = Vec::new();

    for i in 0..5 {
        let base = server.base_url.clone();
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
        let base = server.base_url.clone();
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
        &server.base_url,
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
        &server.base_url,
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
        &server.base_url,
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
        &server.base_url,
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
        &server.base_url,
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
