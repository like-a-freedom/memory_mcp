#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

//! Load test scaffolding for the HTTP SaaS profile.
//!
//! Tests are marked `#[ignore]` and should be run with `--ignored`
//! in CI or manually:
//!
//! ```bash
//! cargo test -p memory_mcp --features streamable-http,test-fixtures \
//!      --test http_load_concurrency -- --ignored
//! ```

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "load test; run with --ignored in CI"]
async fn load_20_active_tenants_under_expected_qps() {
    // TODO: Implement with 20 concurrent tenants, each firing
    // ingest/query requests at expected QPS. Assert:
    // - All requests succeed
    // - Latency stays within bounds
    // - No cross-tenant state leakage
    eprintln!("load_20: placeholder — implement with real server harness");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 32)]
#[ignore = "load test; run with --ignored in CI"]
async fn load_500_tenants_under_contingency_qps() {
    // TODO: Implement with 500 concurrent tenants under contingency
    // load. Assert same invariants as load_20.
    eprintln!("load_500: placeholder — implement with real server harness");
}
