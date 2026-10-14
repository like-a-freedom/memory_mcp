#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

use std::{sync::Arc, time::Duration};

use memory_mcp::http::registry::{
    PrivilegedEngine,
    maintenance_clients::{MaintenanceClientKey, MaintenanceClientPoolConfig, MaintenanceClients},
};
use memory_mcp::storage::DbClient;
use surrealdb::Surreal;
use surrealdb::engine::local::Mem;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;

async fn pool(capacity: usize) -> MaintenanceClients {
    let engine = surrealdb::Surreal::new::<Mem>(())
        .await
        .expect("the embedded fixture starts");
    MaintenanceClients::with_engine_for_test(
        PrivilegedEngine::LocalMem(Arc::new(engine)),
        MaintenanceClientPoolConfig {
            capacity,
            max_metadata_bytes: 1024 * 1024,
            idle_ttl: Duration::from_secs(300),
        },
    )
}

async fn remote_engine() -> PrivilegedEngine {
    let url = std::env::var("MEMORY_MCP_MAINTENANCE_WS_URL")
        .expect("ignored RPC acceptance requires MEMORY_MCP_MAINTENANCE_WS_URL");
    let username = std::env::var("MEMORY_MCP_MAINTENANCE_WS_USERNAME")
        .expect("ignored RPC acceptance requires MEMORY_MCP_MAINTENANCE_WS_USERNAME");
    let password = std::env::var("MEMORY_MCP_MAINTENANCE_WS_PASSWORD")
        .expect("ignored RPC acceptance requires MEMORY_MCP_MAINTENANCE_WS_PASSWORD");
    let db: Surreal<Client> = Surreal::new::<Ws>(url.as_str())
        .await
        .expect("local remote SurrealDB accepts the WebSocket connection");
    db.signin(Root { username, password })
        .await
        .expect("local remote SurrealDB accepts disposable root credentials");
    PrivilegedEngine::Remote(Arc::new(db))
}

fn key(
    namespace: &str,
    database: &str,
    generation: u64,
    dimension: Option<usize>,
) -> MaintenanceClientKey {
    MaintenanceClientKey {
        namespace: namespace.to_owned(),
        database: database.to_owned(),
        engine_generation: generation,
        migration_dimension: dimension,
        requires_ready_schema: true,
    }
}

#[tokio::test]
async fn idle_ticks_reuse_one_bound_session() {
    let pool = pool(32).await;
    let key = key("tns_maintenance_reuse", "memory", 7, None);

    for _ in 0..100 {
        let lease = pool.acquire(key.clone()).await.expect("session binds");
        drop(lease);
    }

    assert_eq!(pool.snapshot().bind_attempts, 1);
    assert_eq!(pool.snapshot().resident_sessions, 1);
    assert_eq!(pool.snapshot().reuse_hits, 99);
}

#[tokio::test]
async fn concurrent_acquires_are_single_flight() {
    let pool = pool(32).await;
    let key = key("tns_maintenance_flight", "memory", 7, None);
    let mut followers = Vec::new();
    for _ in 0..32 {
        let pool = pool.clone();
        let key = key.clone();
        followers.push(tokio::spawn(async move { pool.acquire(key).await }));
    }

    for follower in followers {
        let lease = follower
            .await
            .expect("acquire task")
            .expect("shared binding");
        drop(lease);
    }
    assert_eq!(pool.snapshot().bind_attempts, 1);
    assert_eq!(pool.snapshot().resident_sessions, 1);
}

#[tokio::test]
async fn tenants_and_databases_have_distinct_bound_sessions() {
    let pool = pool(32).await;
    let tenant_a = pool
        .acquire(key("tns_maintenance_a", "memory", 7, None))
        .await
        .expect("tenant A binds");
    let tenant_b = pool
        .acquire(key("tns_maintenance_b", "memory", 7, None))
        .await
        .expect("tenant B binds");
    let database_b = pool
        .acquire(key("tns_maintenance_b", "other", 7, None))
        .await
        .expect("second database binds");

    assert!(!Arc::ptr_eq(&tenant_a.client(), &tenant_b.client()));
    assert!(!Arc::ptr_eq(&tenant_b.client(), &database_b.client()));
    assert_eq!(pool.snapshot().bind_attempts, 3);
}

#[tokio::test]
async fn interleaved_two_tenants_never_cross_namespace_data() {
    let pool = pool(32).await;
    let namespace_a = "tns_maintenance_isolation_a";
    let namespace_b = "tns_maintenance_isolation_b";
    let lease_a = pool
        .acquire(key(namespace_a, "memory", 7, None))
        .await
        .expect("tenant A binds");
    let lease_b = pool
        .acquire(key(namespace_b, "memory", 7, None))
        .await
        .expect("tenant B binds");
    let client_a = lease_a.client();
    let client_b = lease_b.client();

    client_a
        .query(
            "CREATE ONLY maintenance_session_scope:shared CONTENT { owner: 'tenant-a' };",
            None,
            namespace_a,
        )
        .await
        .expect("tenant A writes inside its bound namespace");
    client_b
        .query(
            "CREATE ONLY maintenance_session_scope:shared CONTENT { owner: 'tenant-b' };",
            None,
            namespace_b,
        )
        .await
        .expect("tenant B writes inside its bound namespace");

    let rows_a = client_a
        .query(
            "SELECT * FROM maintenance_session_scope:shared;",
            None,
            namespace_a,
        )
        .await
        .expect("tenant A reads its bound namespace");
    let rows_b = client_b
        .query(
            "SELECT * FROM maintenance_session_scope:shared;",
            None,
            namespace_b,
        )
        .await
        .expect("tenant B reads its bound namespace");

    assert_eq!(rows_a[0]["owner"], "tenant-a");
    assert_eq!(rows_b[0]["owner"], "tenant-b");
}

#[tokio::test]
async fn different_databases_do_not_share_data_or_a_bound_session() {
    let pool = pool(32).await;
    let namespace = "tns_maintenance_database_isolation";
    let lease_a = pool
        .acquire(key(namespace, "maintenance_db_a", 7, None))
        .await
        .expect("database A binds");
    let lease_b = pool
        .acquire(key(namespace, "maintenance_db_b", 7, None))
        .await
        .expect("database B binds");
    let client_a = lease_a.client();
    let client_b = lease_b.client();

    client_a
        .query(
            "CREATE ONLY maintenance_session_scope:shared CONTENT { owner: 'database-a' };",
            None,
            namespace,
        )
        .await
        .expect("database A writes inside its bound database");
    client_b
        .query(
            "CREATE ONLY maintenance_session_scope:shared CONTENT { owner: 'database-b' };",
            None,
            namespace,
        )
        .await
        .expect("database B writes inside its bound database");

    let rows_a = client_a
        .query(
            "SELECT * FROM maintenance_session_scope:shared;",
            None,
            namespace,
        )
        .await
        .expect("database A reads its bound database");
    let rows_b = client_b
        .query(
            "SELECT * FROM maintenance_session_scope:shared;",
            None,
            namespace,
        )
        .await
        .expect("database B reads its bound database");

    assert_eq!(rows_a[0]["owner"], "database-a");
    assert_eq!(rows_b[0]["owner"], "database-b");
}

#[tokio::test]
async fn embedding_dimension_and_engine_generation_prevent_stale_reuse() {
    let pool = pool(8).await;
    let lexical = pool
        .acquire(key("tns_dimension_change", "memory", 3, None))
        .await
        .expect("lexical binding");
    let embedded = pool
        .acquire(key("tns_dimension_change", "memory", 3, Some(2048)))
        .await
        .expect("dimensioned binding");
    let reconnected = pool
        .acquire(key("tns_dimension_change", "memory", 4, Some(2048)))
        .await
        .expect("new engine generation binding");

    assert!(!Arc::ptr_eq(&lexical.client(), &embedded.client()));
    assert!(!Arc::ptr_eq(&embedded.client(), &reconnected.client()));
    assert_eq!(pool.snapshot().bind_attempts, 3);
}

#[tokio::test]
async fn unverified_namespace_bindings_do_not_share_ready_schema_sessions() {
    let pool = pool(8).await;
    let ready_key = key("tns_readiness_identity", "memory", 7, None);
    let raw_key = MaintenanceClientKey {
        requires_ready_schema: false,
        ..ready_key.clone()
    };
    let ready = pool.acquire(ready_key).await.expect("ready bind");
    let raw = pool.acquire(raw_key).await.expect("raw bind");

    assert!(!Arc::ptr_eq(&ready.client(), &raw.client()));
    assert_eq!(pool.snapshot().bind_attempts, 2);
}

#[tokio::test]
async fn one_thousand_distinct_bindings_stay_within_session_and_metadata_bounds() {
    let pool = pool(32).await;
    for index in 0..1000 {
        let lease = pool
            .acquire(key(&format!("tns_churn_{index}"), "memory", 1, None))
            .await
            .expect("bounded pool recycles an idle binding");
        drop(lease);
    }

    let snapshot = pool.snapshot();
    assert_eq!(snapshot.resident_sessions, 32);
    assert!(snapshot.metadata_accounted_bytes <= 1024 * 1024);
    assert_eq!(snapshot.bind_attempts, 1000);
}

#[tokio::test]
#[ignore = "requires the isolated local WebSocket RPC capture fixture"]
async fn remote_wire_legacy_binding_baseline_repeats_session_auth_commands() {
    let engine = remote_engine().await;

    for _ in 0..100 {
        let client = engine
            .clone()
            .bind_namespace_with_dimension("main", "main", None)
            .await
            .expect("legacy maintenance bind succeeds");
        drop(client);
    }
}

#[tokio::test]
#[ignore = "requires the isolated local WebSocket RPC capture fixture"]
async fn remote_wire_idle_ticks_reuse_one_bound_session() {
    let engine = remote_engine().await;
    let pool = MaintenanceClients::with_engine_for_test(
        engine,
        MaintenanceClientPoolConfig {
            capacity: 32,
            max_metadata_bytes: 1024 * 1024,
            idle_ttl: Duration::from_secs(300),
        },
    );
    let binding = MaintenanceClientKey {
        namespace: "main".to_owned(),
        database: "main".to_owned(),
        engine_generation: 7,
        migration_dimension: None,
        requires_ready_schema: false,
    };

    for _ in 0..100 {
        let lease = pool
            .acquire(binding.clone())
            .await
            .expect("pooled maintenance binding succeeds");
        drop(lease);
    }

    let snapshot = pool.snapshot();
    assert_eq!(snapshot.bind_attempts, 1);
    assert_eq!(snapshot.reuse_hits, 99);
    assert_eq!(snapshot.resident_sessions, 1);
    pool.shutdown().await;
}

#[tokio::test]
#[ignore = "requires the isolated local WebSocket RPC capture fixture"]
async fn remote_wire_concurrent_acquires_are_single_flight() {
    let pool = Arc::new(MaintenanceClients::with_engine_for_test(
        remote_engine().await,
        MaintenanceClientPoolConfig {
            capacity: 32,
            max_metadata_bytes: 1024 * 1024,
            idle_ttl: Duration::from_secs(300),
        },
    ));
    let gate = Arc::new(tokio::sync::Barrier::new(33));
    let mut callers = Vec::with_capacity(32);

    for _ in 0..32 {
        let pool = Arc::clone(&pool);
        let gate = Arc::clone(&gate);
        callers.push(tokio::spawn(async move {
            gate.wait().await;
            pool.acquire(key("main", "main", 7, None)).await
        }));
    }
    gate.wait().await;

    for caller in callers {
        let lease = caller
            .await
            .expect("remote acquire task completes")
            .expect("shared remote binding succeeds");
        drop(lease);
    }

    let snapshot = pool.snapshot();
    assert_eq!(snapshot.bind_attempts, 1);
    assert_eq!(snapshot.resident_sessions, 1);
    assert_eq!(snapshot.reuse_hits, 31);
    pool.shutdown().await;
}

#[tokio::test]
#[ignore = "requires the isolated local WebSocket RPC capture fixture"]
async fn remote_wire_concurrent_distinct_bindings_stay_within_capacity() {
    let pool = Arc::new(MaintenanceClients::with_engine_for_test(
        remote_engine().await,
        MaintenanceClientPoolConfig {
            capacity: 32,
            max_metadata_bytes: 1024 * 1024,
            idle_ttl: Duration::from_secs(300),
        },
    ));
    let gate = Arc::new(tokio::sync::Barrier::new(33));
    let mut callers = Vec::with_capacity(32);

    for index in 0..32 {
        let pool = Arc::clone(&pool);
        let gate = Arc::clone(&gate);
        callers.push(tokio::spawn(async move {
            gate.wait().await;
            pool.acquire(key(
                &format!("tns_maintenance_parallel_{index}"),
                "main",
                7,
                None,
            ))
            .await
        }));
    }
    gate.wait().await;

    let mut leases = Vec::with_capacity(32);
    for caller in callers {
        leases.push(
            caller
                .await
                .expect("remote acquire task completes")
                .expect("distinct remote binding succeeds"),
        );
    }

    let snapshot = pool.snapshot();
    assert_eq!(snapshot.resident_sessions, 32);
    assert_eq!(snapshot.creating_bindings, 0);
    assert_eq!(snapshot.active_leases, 32);
    assert!(snapshot.metadata_accounted_bytes <= 1024 * 1024);
    assert_eq!(snapshot.bind_attempts, 32);
    drop(leases);
    pool.shutdown().await;
}

#[tokio::test]
#[ignore = "requires the isolated local WebSocket RPC capture fixture"]
async fn remote_wire_churn_stays_within_session_and_metadata_budgets() {
    let pool = MaintenanceClients::with_engine_for_test(
        remote_engine().await,
        MaintenanceClientPoolConfig {
            capacity: 32,
            max_metadata_bytes: 1024 * 1024,
            idle_ttl: Duration::from_secs(300),
        },
    );

    for index in 0..1000 {
        let lease = pool
            .acquire(key(
                &format!("tns_maintenance_wire_churn_{index}"),
                "main",
                7,
                None,
            ))
            .await
            .expect("bounded remote pool recycles an idle session");
        drop(lease);
    }

    let snapshot = pool.snapshot();
    assert_eq!(snapshot.resident_sessions, 32);
    assert!(snapshot.metadata_accounted_bytes <= 1024 * 1024);
    assert_eq!(snapshot.bind_attempts, 1000);
    assert!(snapshot.evictions >= 968);
    pool.shutdown().await;
}

#[tokio::test]
#[ignore = "requires the isolated local WebSocket RPC capture fixture"]
async fn remote_wire_legacy_churn_drops_each_bound_session() {
    let engine = remote_engine().await;

    for index in 0..1000 {
        let client = engine
            .clone()
            .bind_namespace_with_dimension(
                &format!("tns_maintenance_wire_churn_{index}"),
                "main",
                None,
            )
            .await
            .expect("legacy remote bind succeeds");
        drop(client);
    }
}
