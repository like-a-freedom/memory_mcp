//! Durable production-composition coverage (ADR-0053).
//!
//! Two guarantees:
//! 1. The `test-fixtures` Cargo feature never replaces production
//!    storage: a binary built with the feature still fails startup
//!    when the durable control store is unreachable.
//! 2. `HttpProductionComposition` genuinely persists: a full-binary
//!    writer process commits registry data to an embedded RocksDB
//!    path, and a fresh production composition in another process
//!    reads it back.

#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use memory_mcp::http::composition::HttpProductionComposition;
use memory_mcp::http::config::HttpConfig;
use memory_mcp::http::registry::models::TenantStatus;

/// Deterministic bootstrap credential used by the writer process.
/// Its key id is stable, so the reading composition can navigate
/// Account → Tenant without recomputing the bootstrap hash.
const GATE_KEY: &str =
    "mem_sk_ak_cccc0000-0000-4000-8000-000000000000_compositiontest00000000000000000";
const GATE_KEY_ID: &str = "ak_cccc0000-0000-4000-8000-000000000000";

/// Environment identical in shape to the other HTTP suites. The
/// caller supplies the durable control/tenant store URL so the same
/// helper serves the startup-failure gate and the cross-process
/// durability writer.
fn binary_env(store_url: &str) -> Vec<(String, String)> {
    let zeros = "0".repeat(64);
    vec![
        (
            "MEMORY_MCP_HTTP_TEST_BOOTSTRAP".into(),
            format!("composition_gate={GATE_KEY}"),
        ),
        ("MEMORY_MCP_HTTP_BIND".into(), "127.0.0.1:0".into()),
        (
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL".into(),
            "http://localhost:9".into(),
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
        ("SURREALDB_CONTROL_URL".into(), store_url.to_owned()),
        ("SURREALDB_CONTROL_USERNAME".into(), "root".into()),
        ("SURREALDB_CONTROL_PASSWORD".into(), "root".into()),
        ("SURREALDB_CONTROL_DB".into(), "control".into()),
        ("SURREALDB_CONTROL_NAMESPACE".into(), "control".into()),
        ("SURREALDB_TENANT_URL".into(), store_url.to_owned()),
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
    ]
}

#[test]
fn production_binary_does_not_select_fixture_storage() {
    let env = binary_env("rocksdb:///proc/definitely-missing/rocks");
    let mut child = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"))
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn memory_mcp_http");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait().expect("poll server process") {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "server stayed up with an unreachable durable control store: \
                     test-fixtures must not replace production storage"
                );
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }

    let output = child.wait_with_output().expect("collect server output");
    assert!(
        !output.status.success(),
        "test-fixtures must not replace production storage"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("registry") || stderr.contains("storage"),
        "startup failure must name the storage cause, got: {stderr}"
    );
}

#[tokio::test]
async fn durable_composition_reads_data_written_by_another_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("rocksdb://{}", dir.path().display());

    // Writer: the full binary composes production adapters against the
    // temp RocksDB path and seeds a Ready tenant through the
    // deterministic bootstrap. Process death releases the engine lock
    // deterministically, so the reader never races a half-closed
    // embedded engine.
    let env = binary_env(&url);
    let mut writer = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"))
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn writer binary");
    let stdout = writer.stdout.take().expect("writer stdout");
    let mut bound = false;
    for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)) {
        let line = line.expect("read writer stdout");
        if line.starts_with("memory_mcp_http bound=") {
            bound = true;
            break;
        }
    }
    assert!(bound, "writer binary reported no bound address");
    writer.kill().expect("kill writer");
    writer.wait().expect("reap writer");

    // Reader: a fresh production composition sees the committed signup
    // plan and the bootstrap ApiKey → Account → Tenant chain.
    let mut config = HttpConfig::default_for_test();
    config.control_db.url = url.clone();
    config.control_db.namespace = "control".into();
    config.control_db.database = "control".into();
    config.tenant_db.url = url;
    config.tenant_db.namespace = "tenant".into();
    config.tenant_db.database = "tenant".into();

    let composition = HttpProductionComposition::connect(&config)
        .await
        .expect("composition connects to the durable store another process wrote");
    let store = composition.registry.store_clone();
    let reloaded_plan = store.load_plan(1).await.expect("signup plan is durable");
    assert_eq!(reloaded_plan.version, 1);
    let api_key = store
        .find_api_key(GATE_KEY_ID)
        .await
        .expect("api key lookup")
        .expect("bootstrap api key is durable");
    let account = store
        .find_account_by_id(&api_key.account_id)
        .await
        .expect("account lookup")
        .expect("bootstrap account is durable");
    let reloaded_tenant = store
        .find_tenant_by_id(&account.tenant_id)
        .await
        .expect("tenant lookup")
        .expect("bootstrap tenant is durable");
    assert_eq!(reloaded_tenant.status, TenantStatus::Ready);
}

#[tokio::test]
async fn real_registry_store_admits_ingest_on_mem_engine() {
    use memory_mcp::http::registry::models::{
        Account, AccountStatus, NamespaceBinding, Plan, Tenant, TenantStatus,
    };
    let mut config = HttpConfig::default_for_test();
    config.control_db.url = "mem://".into();
    config.tenant_db.url = "mem://".into();
    let comp = HttpProductionComposition::connect(&config)
        .await
        .expect("mem production connect");
    let plan = Plan {
        id: "free".into(),
        version: 1,
        limits: Default::default(),
    };
    comp.registry.ensure_plan(&plan).await.expect("ensure plan");
    let now = chrono::Utc::now();
    let tenant = Tenant {
        id: "ten_diag".into(),
        status: TenantStatus::Reserved,
        namespace_binding: NamespaceBinding {
            namespace: "tns_diag".into(),
            database: "memory".into(),
        },
        plan_version: 1,
        schema_version: 1,
        retry_stage: None,
        provisioning_lease: None,
        created_at: now,
        version: 0,
    };
    let account = Account {
        id: "acct_diag".into(),
        status: AccountStatus::Active,
        tenant_id: tenant.id.clone(),
        created_at: now,
    };
    comp.registry
        .store_clone()
        .create_account_bundle(&account, &tenant, None)
        .await
        .expect("create bundle");
    let registry_plan = comp
        .registry
        .store_clone()
        .load_plan(1)
        .await
        .expect("load_plan must succeed");
    assert_eq!(registry_plan.version, 1);
    let plan_contract = memory_mcp::http::registry::plan::Plan::from(&registry_plan);
    let decision = comp
        .registry
        .store_clone()
        .reserve_ingest_usage("ten_diag", 1024, &plan_contract, now)
        .await
        .expect("reserve_ingest_usage must succeed");
    assert!(
        matches!(
            decision,
            memory_mcp::http::registry::plan::QuotaDecision::Allow
        ),
        "fresh usage row must admit ingest within quota, got {decision:?}"
    );
}
