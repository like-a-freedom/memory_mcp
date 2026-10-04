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
        // This suite starts a real bound server, so it uses the
        // `local` method: `oidc` runs discovery at startup and would
        // try to reach a placeholder issuer. In `local` mode the three
        // OIDC-typed HMAC keys are derived from the session key, so
        // supplying one here would itself be a refusal.
        ("MEMORY_MCP_HTTP_SESSION_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_CSRF_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_AUTH_METHODS".into(), "local".into()),
        (
            "MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION".into(),
            "1".into(),
        ),
        (
            "MEMORY_MCP_HTTP_MAX_INGESTED_BYTES".into(),
            "1073741824".into(),
        ),
        ("MEMORY_MCP_HTTP_MAX_EPISODE_COUNT".into(), "100000".into()),
        ("MEMORY_MCP_HTTP_INGEST_PER_MINUTE".into(), "60".into()),
        ("MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS".into(), "32".into()),
        ("MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS".into(), "5".into()),
        (
            "MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY".into(),
            "4".into(),
        ),
        ("MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY".into(), "2".into()),
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
    let stores = composition.registry.stores().clone();
    let reloaded_plan = stores
        .usage()
        .load_plan(1)
        .await
        .expect("signup plan is durable");
    assert_eq!(reloaded_plan.version, 1);
    let api_key = stores
        .api_keys()
        .find_api_key(GATE_KEY_ID)
        .await
        .expect("api key lookup")
        .expect("bootstrap api key is durable");
    let account = stores
        .accounts()
        .find_account_by_id(&api_key.account_id)
        .await
        .expect("account lookup")
        .expect("bootstrap account is durable");
    let reloaded_tenant = stores
        .tenants()
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
        display_name: None,
    };
    let bundle_tx =
        memory_mcp::http::registry::control_impl::account_bundle_tx(comp.registry.stores());
    bundle_tx
        .create_account_bundle(&account, &tenant, None)
        .await
        .expect("create bundle");
    let registry_plan = comp
        .registry
        .usage()
        .load_plan(1)
        .await
        .expect("load_plan must succeed");
    assert_eq!(registry_plan.version, 1);
    let plan_contract = memory_mcp::operations::quota::QuotaPlan::from(&registry_plan);
    let decision = comp
        .registry
        .usage()
        .reserve_ingest_usage("ten_diag", 1024, &plan_contract, now)
        .await
        .expect("reserve_ingest_usage must succeed");
    assert!(
        matches!(
            decision,
            memory_mcp::operations::quota::QuotaDecision::Allow
        ),
        "fresh usage row must admit ingest within quota, got {decision:?}"
    );
}

/// The durable quota predicate and the context policy cannot drift.
///
/// ADR-0066 requires this test, and it exists because the two genuinely
/// differ in language. The admission gate is a SQL `WHERE` — it has to be,
/// because the counter increment is conditional on it and a check-then-write
/// would let two racing requests both through. The Rust function exists only
/// to name the refusal, because SQL cannot produce a typed reason carrying a
/// `retry_after_secs`.
///
/// A first version of this test compared the two by *calling* them, and it
/// passed against a deliberately corrupted reason string — because the
/// durable path names its refusal by calling the same function, so the two
/// sides agreed on a corrupted string. That is a tautology: it asserted that
/// a function equals itself. This version reads the SQL predicate's text out
/// of the store's source and requires every reason the policy can produce to
/// have a predicate in it, and every predicate in it to have a reason in the
/// policy. The two can now only agree by actually agreeing.
#[test]
fn the_sql_predicate_covers_every_reason_the_policy_can_produce() {
    use memory_mcp::operations::quota::{QuotaDecision, QuotaPlan, enforce_ingest};

    let store_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http/registry/surreal_store.rs"),
    )
    .expect("the durable store source is readable");

    // The admission statement, isolated from the UPSERT that initialises the
    // row and from the loop around it.
    let predicate = store_source
        .split("UPDATE type::record($table, $tenant_id) SET ingest_window_start")
        .nth(1)
        .expect("the conditional UPDATE is still in the durable store")
        .split("RETURN AFTER")
        .next()
        .expect("the UPDATE ends with RETURN AFTER");

    // Every reason the policy can produce, discovered by driving it rather
    // than by reading its source: a reason the policy stops being able to
    // produce does not need a predicate, and this way the test cannot go
    // stale when the policy changes.
    let mut expected: Vec<String> = Vec::new();
    for (limits, counter, bytes) in reasons_to_try() {
        let quota = QuotaPlan {
            ingest_per_minute: limits.0,
            max_ingested_bytes: limits.1,
            max_episode_count: limits.2,
            ..QuotaPlan::default()
        };
        let now = chrono::Utc::now();
        let mut probe = counter;
        if let QuotaDecision::Deny { reason, .. } = enforce_ingest(&quota, &mut probe, bytes, now) {
            expected.push(reason);
        }
    }
    expected.sort();
    expected.dedup();
    assert!(
        expected.len() >= 4,
        "expected the policy to produce at least its four ceilings, got {expected:?}"
    );

    // Only the `WHERE` clause is the predicate. The `SET` half of the same
    // statement mentions the columns too — it increments them — and a search
    // over the whole statement would find every reason's column there and pass
    // against a `WHERE` that checks nothing. That is not a hypothetical: the
    // first version of this test searched the whole statement and passed with
    // `episode_count < $max_episodes` renamed in the `WHERE` clause only.
    let predicate = predicate
        .split_once(" WHERE ")
        .map(|(_, tail)| tail)
        .expect("the conditional UPDATE has a WHERE clause — it is the admission gate");

    for reason in &expected {
        let needle = match reason.as_str() {
            "ingest_disabled" => "ingest_current_minute < $per_minute",
            "ingested_bytes_exceeded" => "ingested_bytes + $bytes <= $max_bytes",
            "episode_count_exceeded" => "episode_count < $max_episodes",
            "ingest_rate_exceeded" => "ingest_window_start <= type::datetime($cutoff)",
            other => panic!(
                "the policy produced `{other}`, which this test does not know how \
                 to look for in the SQL. Add it to the mapping — a reason the test \
                 cannot pin is a reason the two expressions can drift on silently."
            ),
        };
        assert!(
            predicate.contains(needle),
            "the policy can refuse with `{reason}` but the durable predicate has \
             no `{needle}`. The SQL is the admission gate, so a reason the SQL \
             cannot produce is one a tenant receives as a refusal for some other \
             reason entirely. Predicate was:\n{predicate}"
        );
    }

    // And the rate limit has two clauses in the SQL — the count within the
    // window, or a window that has expired. Both must be present, because a
    // SQL with only one of them refuses after a window rolls and admits a
    // burst at the boundary.
    assert!(
        predicate.contains(
            "ingest_current_minute < $per_minute OR ingest_window_start <= type::datetime($cutoff)"
        ),
        "the SQL must admit either an under-limit minute or an expired window. \
         With only the first clause a tenant is refused forever after a window \
         rolls; with only the second a burst inside one window is admitted \
         without limit. Predicate was:\n{predicate}"
    );
}

/// The states that make each ceiling fire: (per_minute, max_bytes, max_episodes),
/// counter, bytes to reserve.
#[allow(clippy::type_complexity)]
fn reasons_to_try() -> Vec<(
    (u32, u64, u64),
    memory_mcp::operations::quota::UsageCounter,
    u64,
)> {
    use memory_mcp::operations::quota::UsageCounter;
    let now = chrono::Utc::now();
    let fresh = || UsageCounter {
        ingest_current_minute: 0,
        window_start: now,
        ingested_bytes: 0,
        episode_count: 0,
    };
    vec![
        // ingest disabled
        ((0, 1000, 1000), fresh(), 1),
        // byte ceiling
        ((10, 0, 1000), fresh(), 1),
        // episode ceiling
        ((10, 1000, 0), fresh(), 1),
        // rate ceiling: a minute already at its limit
        (
            (1, 1000, 1000),
            UsageCounter {
                ingest_current_minute: 1,
                window_start: now,
                ingested_bytes: 0,
                episode_count: 0,
            },
            1,
        ),
    ]
}
