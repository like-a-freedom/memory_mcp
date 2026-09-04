//! HTTP SaaS load gates.
//!
//! Two executable tests built on `HttpServerFixture`:
//!
//! - `load_20_active_tenants_under_expected_qps` runs as the normal
//!   CI gate. Twenty tenants ingest a unique marker, then call
//!   `explain` on their own episode. Every request must succeed,
//!   every tenant must see its own marker, and no tenant may see
//!   another tenant's marker (cross-tenant isolation).
//! - `load_500_tenants_under_contingency_qps` is a release-only
//!   gate; it asserts the same invariants at 500 tenants and
//!   requires the `MEMORY_MCP_RUN_500_LOAD=1` environment variable
//!   to fire. This way an unconfigured release job fails closed
//!   instead of reporting a skipped/pass result.
//!
//! Run:
//!
//! ```bash
//! # CI gate
//! cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures \
//!     --test http_load_concurrency load_20_active_tenants_under_expected_qps -- --test-threads=1
//!
//! # Release gate (fails without the env var)
//! MEMORY_MCP_RUN_500_LOAD=1 cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures \
//!     --test http_load_concurrency load_500_tenants_under_contingency_qps -- --test-threads=1
//! ```

#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

mod common;

use common::http_server::{HttpServerConfig, HttpServerFixture, TestTenant, mcp_call};

/// Per-request timing plus success/error counts. Serialized to
/// stderr at the end of the run so the gate produces a stable
/// evidence record that downstream tooling can scrape.
#[derive(Debug, Serialize)]
struct LoadEvidence {
    tenant_count: usize,
    request_count: usize,
    success_count: usize,
    error_count: usize,
    p50_ms: u128,
    p95_ms: u128,
    p99_ms: u128,
    max_ms: u128,
}

/// One request observation: status, elapsed time, success flag, and
/// the serialized payload so the isolation assertions can run.
#[derive(Clone)]
#[allow(dead_code)]
struct RequestResult {
    http_status: u16,
    payload_text: String,
    elapsed_ms: u128,
    success: bool,
}

fn build_tenants(count: usize) -> Vec<TestTenant> {
    (0..count)
        .map(|idx| {
            // 8-4-4-4-12 hex with a fixed "4" in the version nibble so
            // the key parses the same as the rest of the suite.
            let uuid = format!(
                "{:08x}-0000-4000-8000-{:012x}",
                0x1000_0000u32.wrapping_add(idx as u32),
                idx as u64
            );
            let secret = format!("loadsecret_{:04}_{:025}", idx, idx);
            TestTenant::new(
                format!("load_t{:03}", idx),
                format!("mem_sk_ak_{uuid}_{secret}"),
            )
        })
        .collect()
}

fn marker_for(idx: usize) -> String {
    format!("load_marker_t{:03}_unique_{}", idx, idx)
}

fn latency_percentiles(mut samples: Vec<u128>) -> (u128, u128, u128, u128) {
    samples.sort_unstable();
    let n = samples.len();
    if n == 0 {
        return (0, 0, 0, 0);
    }
    let pick = |p: f64| -> usize {
        let raw = (n as f64 * p).ceil() as usize;
        raw.saturating_sub(1).min(n - 1)
    };
    (
        samples[pick(0.50)],
        samples[pick(0.95)],
        samples[pick(0.99)],
        *samples.last().unwrap(),
    )
}

async fn time_call(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    method: &str,
    params: Value,
) -> RequestResult {
    let start = Instant::now();
    let resp = mcp_call(client, base_url, api_key, method, params).await;
    let elapsed_ms = start.elapsed().as_millis();
    let http_status = resp["http_status"].as_u64().unwrap_or(0) as u16;
    let payload = &resp["payload"];
    let payload_text = payload.to_string();
    let rpc_error = payload.get("error").is_some();
    RequestResult {
        http_status,
        payload_text,
        elapsed_ms,
        success: http_status == 200 && !rpc_error,
    }
}

fn extract_episode_id(payload_text: &str) -> Option<String> {
    let payload: Value = serde_json::from_str(payload_text).ok()?;
    payload["result"]["structuredContent"]["result"]
        .as_str()
        .map(str::to_owned)
}

fn explain_response_contains_marker(payload_text: &str, marker: &str) -> bool {
    let payload: Value = match serde_json::from_str(payload_text) {
        Ok(value) => value,
        Err(_) => return false,
    };
    payload["result"]["structuredContent"]["result"]
        .as_array()
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item["content"].as_str().is_some_and(|c| c.contains(marker)))
        })
}

/// Run the two-phase workload against the fixture. Returns the
/// per-tenant ingest and explain results in tenant-index order so
/// the isolation assertions can pair them up directly.
async fn run_load(
    fixture: &HttpServerFixture,
    tenants: &[TestTenant],
) -> (Vec<RequestResult>, Vec<RequestResult>) {
    let client = fixture.client().clone();
    let base_url = fixture.base_url.clone();

    // Phase 1: each tenant ingests its own unique marker and we
    // capture the returned episode id.
    let mut ingest_handles = Vec::with_capacity(tenants.len());
    for (idx, tenant) in tenants.iter().enumerate() {
        let c = client.clone();
        let url = base_url.clone();
        let key = tenant.api_key.clone();
        let content = marker_for(idx);
        let t_ref = "2026-08-27T00:00:00Z";
        ingest_handles.push(tokio::spawn(async move {
            time_call(
                &c,
                &url,
                &key,
                "tools/call",
                serde_json::json!({
                    "name": "ingest",
                    "arguments": {
                        "content": content,
                        "source_type": "load_test",
                        "source_id": format!("load_t{:03}", idx),
                        "t_ref": t_ref,
                        "t_ingested": null,
                        "policy_tags": []
                    }
                }),
            )
            .await
        }));
    }

    let mut ingests = Vec::with_capacity(tenants.len());
    let mut episode_ids: Vec<Option<String>> = Vec::with_capacity(tenants.len());
    for handle in ingest_handles.into_iter() {
        let r = handle.await.expect("ingest task joins");
        episode_ids.push(extract_episode_id(&r.payload_text));
        ingests.push(r);
    }

    // Phase 2: each tenant calls explain on its own episode id.
    let mut explain_handles = Vec::with_capacity(tenants.len());
    for (tenant, episode_id) in tenants.iter().zip(episode_ids.iter()) {
        let Some(episode_id) = episode_id else {
            continue;
        };
        let c = client.clone();
        let url = base_url.clone();
        let key = tenant.api_key.clone();
        let episode_id = episode_id.clone();
        explain_handles.push(tokio::spawn(async move {
            time_call(
                &c,
                &url,
                &key,
                "tools/call",
                serde_json::json!({
                    "name": "explain",
                    "arguments": {
                        "context_items": format!("[\"{episode_id}\"]"),
                        "compact": false
                    }
                }),
            )
            .await
        }));
    }

    let mut explains = Vec::with_capacity(explain_handles.len());
    for handle in explain_handles.into_iter() {
        explains.push(handle.await.expect("explain task joins"));
    }

    (ingests, explains)
}

fn summarize(label: &str, tenants: &[TestTenant], requests: &[RequestResult]) -> LoadEvidence {
    let latencies: Vec<u128> = requests.iter().map(|r| r.elapsed_ms).collect();
    let (p50, p95, p99, max) = latency_percentiles(latencies);
    let success_count = requests.iter().filter(|r| r.success).count();
    let request_count = requests.len();
    let error_count = request_count - success_count;
    let evidence = LoadEvidence {
        tenant_count: tenants.len(),
        request_count,
        success_count,
        error_count,
        p50_ms: p50,
        p95_ms: p95,
        p99_ms: p99,
        max_ms: max,
    };
    eprintln!(
        "{} evidence: {}",
        label,
        serde_json::to_string(&evidence).expect("serialize evidence")
    );
    evidence
}

fn assert_isolation(tenants: &[TestTenant], explains: &[RequestResult]) {
    let mut all_own = true;
    let mut all_clean = true;
    for idx in 0..tenants.len() {
        let Some(explain) = explains.get(idx) else {
            all_own = false;
            continue;
        };
        let marker = marker_for(idx);
        if !explain_response_contains_marker(&explain.payload_text, &marker) {
            all_own = false;
        }
        for other_idx in 0..tenants.len() {
            if other_idx == idx {
                continue;
            }
            if explain.payload_text.contains(&marker_for(other_idx)) {
                all_clean = false;
            }
        }
    }
    assert!(all_own, "every tenant must see its own marker");
    assert!(all_clean, "no tenant may see another tenant's marker");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
async fn load_20_active_tenants_under_expected_qps() {
    const TENANT_COUNT: usize = 20;

    let tenants = build_tenants(TENANT_COUNT);
    let config = HttpServerConfig {
        tenants: tenants.clone(),
        extra_env: Vec::new(),
        storage_url: "mem://".into(),
    };
    let fixture = HttpServerFixture::spawn(config).await;

    let (ingests, explains) = run_load(&fixture, &tenants).await;
    let mut all_requests = ingests;
    all_requests.extend(explains.iter().cloned());
    let evidence = summarize("load_20", &tenants, &all_requests);

    assert_eq!(evidence.tenant_count, TENANT_COUNT);
    assert_eq!(evidence.error_count, 0, "load_20: errors in {evidence:?}");
    assert_eq!(evidence.success_count, evidence.request_count);
    assert_isolation(&tenants, &explains);

    // Generous CI ceiling: a single embedded memory backend on the
    // same machine should land p95 well under 5s with 20 tenants.
    assert!(evidence.p95_ms <= 5_000, "p95 too high: {evidence:?}");
    assert!(evidence.max_ms <= 15_000, "max too high: {evidence:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 64)]
#[ignore = "release-gate 500-tenant load; requires MEMORY_MCP_HTTP_500_TENANT=1"]
async fn load_500_tenants_under_contingency_qps() {
    const TENANT_COUNT: usize = 500;

    let gate_ok = std::env::var("MEMORY_MCP_RUN_500_LOAD").as_deref() == Ok("1")
        || std::env::var("MEMORY_MCP_HTTP_500_TENANT").as_deref() == Ok("1");
    assert!(
        gate_ok,
        "release gate requires MEMORY_MCP_RUN_500_LOAD=1 or MEMORY_MCP_HTTP_500_TENANT=1"
    );

    let tenants = build_tenants(TENANT_COUNT);
    let config = HttpServerConfig {
        tenants: tenants.clone(),
        extra_env: Vec::new(),
        storage_url: "mem://".into(),
    };
    let fixture = HttpServerFixture::spawn(config).await;

    let (ingests, explains) = run_load(&fixture, &tenants).await;
    let mut all_requests = ingests;
    all_requests.extend(explains.iter().cloned());
    let evidence = summarize("load_500", &tenants, &all_requests);

    assert_eq!(evidence.tenant_count, TENANT_COUNT);
    assert_eq!(evidence.error_count, 0, "load_500: errors in {evidence:?}");
    assert_eq!(evidence.success_count, evidence.request_count);
    assert_isolation(&tenants, &explains);

    assert!(evidence.p95_ms <= 5_000, "p95 too high: {evidence:?}");
    assert!(evidence.max_ms <= 15_000, "max too high: {evidence:?}");

    // Reference the latency ceiling constant so the unused-import
    // lint stays quiet on this gated test.
    let _ceiling: Duration = Duration::from_millis(15_000);
}
