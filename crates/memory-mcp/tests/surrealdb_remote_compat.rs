use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{TimeZone, Utc};
use memory_mcp::config::{ActiveNamespace, SurrealConfig};
use memory_mcp::knowledge::KnowledgeStoreClient;
use memory_mcp::models::Provenance;
use memory_mcp::service::MemoryService;
use memory_mcp::storage::{ContextFactQuery, DbClient, SurrealDbClient};

#[cfg(all(feature = "streamable-http", feature = "test-fixtures"))]
use memory_mcp::config::SurrealTargetConfig;
#[cfg(all(feature = "streamable-http", feature = "test-fixtures"))]
use memory_mcp::http::leases::migration::{ApplyMigrations, SurrealTenantMigrations};
#[cfg(all(feature = "streamable-http", feature = "test-fixtures"))]
use memory_mcp::http::registry::SurrealRegistryStore;

#[cfg(all(feature = "streamable-http", feature = "test-fixtures"))]
#[path = "common/http_server.rs"]
mod http_server;

fn isolated_remote_config() -> (SurrealConfig, String) {
    let expected_version = std::env::var("MEMORY_SURREALDB_EXPECTED_VERSION")
        .expect("MEMORY_SURREALDB_EXPECTED_VERSION must be set");
    let mut config = SurrealConfig::from_env().expect("remote SurrealDB config");
    assert!(
        !config.embedded,
        "compatibility test must use a remote server"
    );
    let url = config.url.as_deref().expect("remote URL");
    assert!(
        url.starts_with("ws://127.0.0.1:")
            || url.starts_with("ws://localhost:")
            || url.starts_with("ws://[::1]:"),
        "remote compatibility tests only allow a loopback URL"
    );

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    config.namespace =
        ActiveNamespace::new(format!("compat_{suffix}")).expect("generated namespace is valid");
    config.db_name = format!("compat_{suffix}");
    (config, expected_version)
}

fn run_primary_maintenance(config: &SurrealConfig, allow_remote_v4: bool) -> Output {
    Command::new(env!("CARGO_BIN_EXE_memory_mcp"))
        .args(["surrealdb-v4-maintenance", "--role", "primary"])
        .env("SURREALDB_URL", config.url.as_deref().expect("remote URL"))
        .env("SURREALDB_EMBEDDED", "false")
        .env("SURREALDB_DB_NAME", &config.db_name)
        .env("SURREALDB_NAMESPACE", config.namespace.as_str())
        .env("SURREALDB_USERNAME", &config.username)
        .env("SURREALDB_PASSWORD", &config.password)
        .env(
            "SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD",
            allow_remote_v4.to_string(),
        )
        .env("SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X", "true")
        .output()
        .expect("run primary maintenance command")
}

#[tokio::test]
#[ignore = "requires an isolated loopback SurrealDB 3.3.5 or 4.0.1 server"]
async fn authenticated_remote_client_reports_the_pinned_server_version() {
    let (config, expected_version) = isolated_remote_config();
    let client = SurrealDbClient::connect(&config)
        .await
        .expect("authenticated remote connection");
    let version = client
        .server_version(config.namespace.as_str())
        .await
        .expect("server version query")
        .expect("server version is available");

    assert_eq!(version, format!("SurrealDB {expected_version}"));
}

#[tokio::test]
#[ignore = "requires an isolated loopback SurrealDB 3.3.5 or 4.0.1 server"]
async fn primary_catalog_maintenance_is_version_aware_and_migrates_the_remote_database() {
    let (config, expected_version) = isolated_remote_config();
    let client = SurrealDbClient::connect(&config)
        .await
        .expect("authenticated remote connection");
    let version = client
        .server_version(config.namespace.as_str())
        .await
        .expect("server version query")
        .expect("server version is available");
    assert_eq!(version, format!("SurrealDB {expected_version}"));

    if expected_version.starts_with('4') {
        let refused = run_primary_maintenance(&config, false);
        assert!(!refused.status.success());
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&refused.stdout),
            String::from_utf8_lossy(&refused.stderr)
        );
        assert!(
            output.contains("SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD"),
            "unexpected refusal: {output}"
        );
        let untouched = client
            .query("INFO FOR DB;", None, config.namespace.as_str())
            .await
            .expect("read database after denied maintenance")
            .to_string();
        assert!(
            !untouched.contains("memory_v4_maintenance_claim") && !untouched.contains("episode"),
            "authorization refusal must happen before remote catalog SQL: {untouched}"
        );
    }

    let maintenance = run_primary_maintenance(&config, expected_version.starts_with('4'));
    assert!(
        maintenance.status.success(),
        "primary maintenance failed: {}{}",
        String::from_utf8_lossy(&maintenance.stdout),
        String::from_utf8_lossy(&maintenance.stderr)
    );

    let catalog = client
        .query("INFO FOR DB;", None, config.namespace.as_str())
        .await
        .expect("primary catalog exists after maintenance");
    let catalog = catalog.to_string();
    for table in [
        "episode",
        "entity",
        "fact",
        "edge",
        "community",
        "event_log",
        "task",
        "script_migration",
        "query_log",
        "embedding_state",
        "embedding_job",
        "triple",
        "memory_event",
        "event_projection_job",
        "memory_capture_audit",
        "procedure_candidate",
        "claim",
        "claim_relation",
        "claim_job",
        "claim_key_alias",
        "claim_policy",
        "entity_extraction_projection",
        "inbox_revision",
    ] {
        assert!(
            catalog.contains(table),
            "primary catalog missing {table}: {catalog}"
        );
    }

    let record_id = format!("episode:remote_compat_{}", config.db_name);
    let timestamp = "2026-11-18T12:34:56Z";
    client
        .create(
            &record_id,
            serde_json::json!({
                "episode_id": record_id,
                "source_type": "compatibility-test",
                "source_id": config.db_name,
                "content": "SurrealDB remote round trip",
                "t_ref": timestamp,
                "t_ingested": timestamp,
                "scope": "global",
                "visibility_scope": "private",
                "policy_tags": [],
            }),
            config.namespace.as_str(),
            &["t_ref", "t_ingested"],
        )
        .await
        .expect("create remote episode with datetime fields");
    let stored = client
        .select_one(&record_id, config.namespace.as_str())
        .await
        .expect("read remote episode")
        .expect("episode round-trips");
    let stored_time = chrono::DateTime::parse_from_rfc3339(
        stored["t_ref"].as_str().expect("datetime is serialized"),
    )
    .expect("stored datetime is RFC3339");
    assert_eq!(
        stored_time,
        chrono::DateTime::parse_from_rfc3339(timestamp).unwrap()
    );
}

#[cfg(all(feature = "streamable-http", feature = "test-fixtures"))]
#[tokio::test]
#[ignore = "requires an isolated loopback SurrealDB 3.3.5 or 4.0.1 server"]
async fn http_maintenance_migrates_registry_and_a_ready_tenant() {
    use std::process::Command;

    let (config, expected_version) = isolated_remote_config();
    let url = config.url.as_deref().expect("remote URL").to_owned();
    let control_target = SurrealTargetConfig {
        url: url.clone(),
        username: config.username.clone(),
        password: config.password.clone(),
        database: config.db_name.clone(),
        namespace: config.namespace.as_str().to_owned(),
    };
    let suffix = config.db_name.strip_prefix("compat_").expect("suffix");
    let tenant_namespace = format!("tns_compat_{suffix}");
    let tenant_database = format!("db_compat_{suffix}");
    let tenant_target = SurrealTargetConfig {
        url: url.clone(),
        username: config.username.clone(),
        password: config.password.clone(),
        database: format!("base_{suffix}"),
        namespace: format!("base_{suffix}"),
    };

    let http_config = http_server::HttpServerConfig::default()
        .with_env("SURREALDB_CONTROL_URL", url.clone())
        .with_env("SURREALDB_CONTROL_USERNAME", config.username.clone())
        .with_env("SURREALDB_CONTROL_PASSWORD", config.password.clone())
        .with_env("SURREALDB_CONTROL_DB", control_target.database.clone())
        .with_env(
            "SURREALDB_CONTROL_NAMESPACE",
            control_target.namespace.clone(),
        )
        .with_env("SURREALDB_TENANT_URL", url)
        .with_env("SURREALDB_TENANT_USERNAME", config.username.clone())
        .with_env("SURREALDB_TENANT_PASSWORD", config.password.clone())
        .with_env("SURREALDB_TENANT_DB", tenant_target.database.clone())
        .with_env(
            "SURREALDB_TENANT_NAMESPACE",
            tenant_target.namespace.clone(),
        )
        .with_env("MEMORY_MCP_HTTP_TEST_BOOTSTRAP", "remote_compat=unused")
        .with_env("SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X", "true")
        .with_env(
            "SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD",
            expected_version.starts_with('4').to_string(),
        );
    let run_maintenance = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
        command
            .arg("surrealdb-v4-maintenance")
            .env("RUST_LOG", "error");
        command.envs(http_server::build_env(&http_config, "mem://"));
        command
    };
    let first_run = run_maintenance()
        .output()
        .expect("initialize remote HTTP registry");
    assert!(
        first_run.status.success(),
        "initial HTTP maintenance failed: {}{}",
        String::from_utf8_lossy(&first_run.stdout),
        String::from_utf8_lossy(&first_run.stderr)
    );
    let first_result: serde_json::Value =
        serde_json::from_slice(&first_run.stdout).expect("maintenance returns JSON");
    assert_eq!(first_result["ready_tenants"], 0, "{first_result}");

    let registry = SurrealDbClient::connect_bound(&control_target, "error")
        .await
        .expect("bind migrated registry for fixture setup");
    let registry_schema = registry
        .query("INFO FOR DB;", None, &control_target.namespace)
        .await
        .expect("read migrated registry schema")
        .to_string();
    for table in [
        "account",
        "tenant",
        "api_key",
        "external_identity",
        "provisioning_event",
        "plan",
        "deletion_challenge",
        "usage",
        "control_plane_session",
        "oidc_request",
        "audit_event",
        "migration_ledger",
        "browser_auth_policy",
        "local_admin",
        "local_admin_challenge",
        "local_admin_session",
        "local_admin_rate_bucket",
        "local_admin_client",
        "local_admin_client_key",
        "local_admin_operation",
        "local_admin_audit",
    ] {
        assert!(
            registry_schema.contains(table),
            "registry catalog missing {table}: {registry_schema}"
        );
    }

    let engine = SurrealRegistryStore::connect_engine(&tenant_target)
        .await
        .expect("connect privileged tenant engine");
    let migrations = SurrealTenantMigrations::new(engine)
        .with_remote_v4_index_rebuild_ack(expected_version.starts_with('4'));
    migrations
        .ensure_namespace(&tenant_namespace, &tenant_database)
        .await
        .expect("create the tenant namespace and database");
    let tenant_id = format!("ten_compat_{suffix}");
    registry
        .query(
            "CREATE type::record($table, $id) SET id = $id, status = 'ready', namespace_binding = $binding, plan_version = 1, schema_version = 0, retry_stage = NONE, provisioning_lease = NONE, created_at = type::datetime($created_at), version = 0;",
            Some(serde_json::json!({
                "table": "tenant",
                "id": tenant_id,
                "binding": { "namespace": tenant_namespace, "database": tenant_database },
                "created_at": chrono::Utc::now().to_rfc3339(),
            })),
            &control_target.namespace,
        )
        .await
        .expect("seed a ready tenant in the remote registry");

    let output = run_maintenance()
        .output()
        .expect("run HTTP v4 maintenance against remote database");
    assert!(
        output.status.success(),
        "HTTP maintenance failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("maintenance returns JSON");
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(result["ready_tenants"], 1, "{result}");
    assert_eq!(result["tenant_databases"], 1, "{result}");

    let tenant = SurrealDbClient::connect_bound(
        &SurrealTargetConfig {
            url: tenant_target.url,
            username: tenant_target.username,
            password: tenant_target.password,
            namespace: tenant_namespace.clone(),
            database: tenant_database,
        },
        "error",
    )
    .await
    .expect("bind tenant after maintenance");
    let schema = tenant
        .query("INFO FOR DB;", None, &tenant_namespace)
        .await
        .expect("read migrated HTTP tenant schema")
        .to_string();
    for table in [
        "fact",
        "app_session",
        "tenant_task",
        "tenant_change_event",
        "task_artifact",
    ] {
        assert!(
            schema.contains(table),
            "tenant catalog missing {table}: {schema}"
        );
    }
}

async fn add_remote_fact(service: &MemoryService, content: &str, day: u32) -> String {
    service
        .add_fact(
            "note",
            content,
            content,
            "episode:remote-compat-retrieval",
            Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0)
                .single()
                .expect("valid fixture date"),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("insert remote retrieval fact")
}

#[tokio::test]
#[ignore = "requires an isolated loopback SurrealDB 3.3.5 or 4.0.1 server"]
async fn remote_retrieval_preserves_bm25_temporal_vector_and_page_ordering() {
    let (config, expected_version) = isolated_remote_config();
    let maintenance = run_primary_maintenance(&config, expected_version.starts_with('4'));
    assert!(
        maintenance.status.success(),
        "primary maintenance failed: {}{}",
        String::from_utf8_lossy(&maintenance.stdout),
        String::from_utf8_lossy(&maintenance.stderr)
    );

    let db = Arc::new(
        SurrealDbClient::connect(&config)
            .await
            .expect("remote client"),
    );
    let service = MemoryService::new(
        db.clone(),
        config.namespace.as_str().to_owned(),
        "ERROR".to_owned(),
        50,
        100,
    )
    .expect("memory service");
    let knowledge = KnowledgeStoreClient::new(db.clone(), config.namespace.as_str());
    let exact = add_remote_fact(
        &service,
        "Orion telescope alignment telemetry calibration is stable",
        3,
    )
    .await;
    let repeated = add_remote_fact(
        &service,
        "Orion telescope telescope alignment telemetry calibration was repeated",
        2,
    )
    .await;
    let temporal_only = add_remote_fact(
        &service,
        "Orion alignment uses a separate navigation sensor",
        1,
    )
    .await;
    let cutoff = "2027-01-01T00:00:00Z";

    let lexical = knowledge
        .select_facts_filtered(ContextFactQuery {
            cutoff,
            query_contains: Some("Orion telescope"),
            limit: 10,
            fact_types: &[],
        })
        .await
        .expect("execute the production BM25 query");
    assert_eq!(lexical.len(), 2, "both matching facts should be retrieved");
    assert!(lexical.iter().any(|row| row["fact_id"] == exact));
    assert!(lexical.iter().any(|row| row["fact_id"] == repeated));
    let scores = lexical
        .iter()
        .map(|row| row["ft_score"].as_f64().expect("BM25 score"))
        .collect::<Vec<_>>();
    assert!(
        scores[0] >= scores[1],
        "BM25 results are score ordered: {scores:?}"
    );

    let temporal = knowledge
        .select_facts_filtered(ContextFactQuery {
            cutoff,
            query_contains: None,
            limit: 2,
            fact_types: &[],
        })
        .await
        .expect("execute the production temporal query");
    assert_eq!(
        temporal
            .iter()
            .map(|row| row["fact_id"].as_str().expect("fact id"))
            .collect::<Vec<_>>(),
        vec![exact.as_str(), repeated.as_str()],
        "temporal results use descending validity time"
    );

    let mut first_page = db
        .query(
            "SELECT VALUE fact_id FROM fact ORDER BY fact_id ASC LIMIT $limit START $start;",
            Some(serde_json::json!({ "limit": 2, "start": 0 })),
            config.namespace.as_str(),
        )
        .await
        .expect("read the first ordered page");
    let mut second_page = db
        .query(
            "SELECT VALUE fact_id FROM fact ORDER BY fact_id ASC LIMIT $limit START $start;",
            Some(serde_json::json!({ "limit": 2, "start": 2 })),
            config.namespace.as_str(),
        )
        .await
        .expect("read the second ordered page");
    let mut paged_ids = first_page
        .as_array_mut()
        .expect("first page is an array")
        .drain(..)
        .chain(
            second_page
                .as_array_mut()
                .expect("second page is an array")
                .drain(..),
        )
        .map(|id| id.as_str().expect("page record id").to_owned())
        .collect::<Vec<_>>();
    let mut expected_ids = vec![exact.clone(), repeated.clone(), temporal_only.clone()];
    expected_ids.sort();
    assert_eq!(
        paged_ids, expected_ids,
        "ordered pages have no gaps or duplicates"
    );
    paged_ids.clear();

    let dimension = memory_mcp::config::DEFAULT_EMBEDDING_DIMENSION;
    let mut aligned = vec![0.0; dimension];
    aligned[0] = 1.0;
    let mut near_tie = vec![0.0_f64; dimension];
    near_tie[0] = 0.999_999;
    near_tie[1] = (1.0_f64 - near_tie[0].powi(2)).sqrt();
    let mut orthogonal = vec![0.0; dimension];
    orthogonal[1] = 1.0;
    for (fact_id, embedding) in [
        (&exact, aligned),
        (&repeated, near_tie),
        (&temporal_only, orthogonal),
    ] {
        let key = fact_id.strip_prefix("fact:").expect("fact record id");
        db.query(
            "UPDATE type::record('fact', $key) SET embedding = $embedding;",
            Some(serde_json::json!({ "key": key, "embedding": embedding })),
            config.namespace.as_str(),
        )
        .await
        .expect("store vector fixture");
    }
    let query_vector = aligned_vector(dimension);
    let semantic = knowledge
        .select_facts_ann(cutoff, &query_vector, 3)
        .await
        .expect("execute the production vector query");
    assert_eq!(
        semantic.len(),
        3,
        "the ANN query returns every fixture candidate"
    );
    let semantic_scores = semantic
        .iter()
        .map(|row| row["sem_score"].as_f64().expect("cosine score"))
        .collect::<Vec<_>>();
    assert!(
        semantic_scores[0] >= semantic_scores[1] && semantic_scores[1] > semantic_scores[2],
        "near-tie candidates remain ordered ahead of the orthogonal candidate: {semantic_scores:?}"
    );
    assert!(
        semantic_scores[0] - semantic_scores[1] < 0.001,
        "the leading candidates are deliberately near ties: {semantic_scores:?}"
    );
}

fn aligned_vector(dimension: usize) -> Vec<f64> {
    let mut vector = vec![0.0; dimension];
    vector[0] = 1.0;
    vector
}
