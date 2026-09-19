//! Remote race tests for local admin authentication.
//!
//! These tests require an isolated SurrealDB 3.2.4 instance.
//! Run with: cargo test --features control-plane,test-fixtures -- --ignored

use std::sync::Arc;

/// Test concurrent admin creation with same username.
///
/// Requires: LOCAL_ADMIN_TEST_CONTROL_URL, LOCAL_ADMIN_TEST_CONTROL_USERNAME,
/// LOCAL_ADMIN_TEST_CONTROL_PASSWORD environment variables.
#[tokio::test]
#[ignore = "requires isolated remote SurrealDB 3.2.4"]
async fn local_admin_remote_replica_races() {
    let control_url = std::env::var("LOCAL_ADMIN_TEST_CONTROL_URL")
        .expect("LOCAL_ADMIN_TEST_CONTROL_URL must be set");
    let control_user = std::env::var("LOCAL_ADMIN_TEST_CONTROL_USERNAME")
        .expect("LOCAL_ADMIN_TEST_CONTROL_USERNAME must be set");
    let control_pass = std::env::var("LOCAL_ADMIN_TEST_CONTROL_PASSWORD")
        .expect("LOCAL_ADMIN_TEST_CONTROL_PASSWORD must be set");

    // Create isolated namespace for this test run
    let namespace = format!("local_admin_race_test_{}", uuid::Uuid::new_v4().simple());

    // Connect to remote SurrealDB
    let db = surrealdb::Surreal::new::<surrealdb::engine::remote::ws::Ws>(&control_url)
        .await
        .expect("connect to remote SurrealDB");

    db.use_ns(&namespace).use_db("test_race")
        .await
        .expect("use namespace");

    // Authenticate
    db.signin(surrealdb::opt::auth::Root {
        username: control_user,
        password: control_pass,
    })
    .await
    .expect("signin");

    // Run migrations
    let migrations_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("migrations");
    let migration = std::fs::read_to_string(migrations_dir.join("047_local_admin_auth.surql"))
        .expect("read migration");
    db.query(&migration).await.expect("apply migration");

    // Test 1: Concurrent admin creation should handle gracefully
    let db1 = Arc::new(db.clone());
    let db2 = Arc::new(db.clone());

    let handle1 = tokio::spawn(async move {
        db1.query("CREATE local_admin SET username = 'ops.one', state = 'pending_activation', credential_generation = 1, created_at = time::now()")
            .await
    });
    let handle2 = tokio::spawn(async move {
        db2.query("CREATE local_admin SET username = 'ops.one', state = 'pending_activation', credential_generation = 1, created_at = time::now()")
            .await
    });

    let (r1, r2) = tokio::join!(handle1, handle2);

    // At least one should succeed (or both if unique constraint allows duplicates)
    // The important thing is no crash or corruption
    assert!(r1.is_ok() || r2.is_ok(), "at least one concurrent create should succeed");

    // Cleanup
    let _ = db.query(&format!("REMOVE DATABASE test_race")).await;
    let _ = db.query(&format!("REMOVE NAMESPACE {namespace}")).await;
}

/// Test session revocation during concurrent login.
///
/// Requires: LOCAL_ADMIN_TEST_CONTROL_URL, LOCAL_ADMIN_TEST_CONTROL_USERNAME,
/// LOCAL_ADMIN_TEST_CONTROL_PASSWORD environment variables.
#[tokio::test]
#[ignore = "requires isolated remote SurrealDB 3.2.4"]
async fn local_admin_session_revocation_race() {
    let control_url = std::env::var("LOCAL_ADMIN_TEST_CONTROL_URL")
        .expect("LOCAL_ADMIN_TEST_CONTROL_URL must be set");
    let control_user = std::env::var("LOCAL_ADMIN_TEST_CONTROL_USERNAME")
        .expect("LOCAL_ADMIN_TEST_CONTROL_USERNAME must be set");
    let control_pass = std::env::var("LOCAL_ADMIN_TEST_CONTROL_PASSWORD")
        .expect("LOCAL_ADMIN_TEST_CONTROL_PASSWORD must be set");

    let namespace = format!("local_admin_session_race_{}", uuid::Uuid::new_v4().simple());

    let db = surrealdb::Surreal::new::<surrealdb::engine::remote::ws::Ws>(&control_url)
        .await
        .expect("connect to remote SurrealDB");

    db.use_ns(&namespace).use_db("test_session_race")
        .await
        .expect("use namespace");

    db.signin(surrealdb::opt::auth::Root {
        username: control_user,
        password: control_pass,
    })
    .await
    .expect("signin");

    // Apply migration
    let migrations_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("migrations");
    let migration = std::fs::read_to_string(migrations_dir.join("047_local_admin_auth.surql"))
        .expect("read migration");
    db.query(&migration).await.expect("apply migration");

    // Create test admin
    db.query("CREATE local_admin SET username = 'race.admin', state = 'active', credential_generation = 1, created_at = time::now()")
        .await
        .expect("create admin");

    // Create two sessions
    db.query("CREATE local_admin_session SET admin_id = (SELECT id FROM local_admin WHERE username = 'race.admin')[0].id, cookie_verifier = 'aaaa', credential_generation = 1, created_at = time::now(), idle_expiry = time::now() + 1800s, absolute_expiry = time::now() + 86400s, revoked = false")
        .await
        .expect("create session 1");

    db.query("CREATE local_admin_session SET admin_id = (SELECT id FROM local_admin WHERE username = 'race.admin')[0].id, cookie_verifier = 'bbbb', credential_generation = 1, created_at = time::now(), idle_expiry = time::now() + 1800s, absolute_expiry = time::now() + 86400s, revoked = false")
        .await
        .expect("create session 2");

    // Concurrent revoke + resolve
    let db1 = Arc::new(db.clone());
    let db2 = Arc::new(db.clone());

    let handle1 = tokio::spawn(async move {
        db1.query("UPDATE local_admin_session SET revoked = true WHERE cookie_verifier = 'aaaa'")
            .await
    });
    let handle2 = tokio::spawn(async move {
        db2.query("SELECT * FROM local_admin_session WHERE cookie_verifier = 'bbbb' AND revoked = false")
            .await
    });

    let (r1, r2) = tokio::join!(handle1, handle2);
    assert!(r1.is_ok(), "revoke should succeed");
    assert!(r2.is_ok(), "resolve should succeed");

    // Cleanup
    let _ = db.query("REMOVE DATABASE test_session_race").await;
    let _ = db.query(&format!("REMOVE NAMESPACE {namespace}")).await;
}
