#![cfg(all(feature = "streamable-http", feature = "control-plane"))]

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

    db.use_ns(&namespace)
        .use_db("test_race")
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
    let migrations_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let migration = std::fs::read_to_string(migrations_dir.join("047_local_admin_auth.surql"))
        .expect("read migration");
    db.query(&migration).await.expect("apply migration");

    // Test 1: Concurrent admin creation of the same unique username.
    //
    // `local_admin` is SCHEMAFULL and requires `id` (string, unique index),
    // `version` and `updated_at`; both inserts therefore supply every field.
    // Distinct record ids keep the conflict on the UNIQUE `username` index so
    // exactly one insert may commit (statement errors surface through
    // `Response::take_errors`, not through the outer `Result`).
    let db1 = Arc::new(db.clone());
    let db2 = Arc::new(db.clone());

    let handle1 = tokio::spawn(async move {
        db1.query("CREATE type::record('local_admin', 'race_one') SET id = 'race_one', username = 'ops.one', state = 'pending_activation', password_phc = NONE, credential_generation = 1, version = 1, created_at = time::now(), updated_at = time::now()")
            .await
    });
    let handle2 = tokio::spawn(async move {
        db2.query("CREATE type::record('local_admin', 'race_two') SET id = 'race_two', username = 'ops.one', state = 'pending_activation', password_phc = NONE, credential_generation = 1, version = 1, created_at = time::now(), updated_at = time::now()")
            .await
    });

    let (r1, r2) = tokio::join!(handle1, handle2);

    // Exactly one concurrent create of the same username must commit; the
    // other is rejected by the UNIQUE username index. Neither may crash.
    let committed = usize::from(
        r1.expect("create task 1 joined")
            .is_ok_and(|mut response| response.take_errors().is_empty()),
    ) + usize::from(
        r2.expect("create task 2 joined")
            .is_ok_and(|mut response| response.take_errors().is_empty()),
    );
    assert_eq!(
        committed, 1,
        "exactly one concurrent create of the same username should succeed"
    );

    // Cleanup
    let _ = db.query("REMOVE DATABASE test_race").await;
    let _ = db.query(format!("REMOVE NAMESPACE {namespace}")).await;
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

    db.use_ns(&namespace)
        .use_db("test_session_race")
        .await
        .expect("use namespace");

    db.signin(surrealdb::opt::auth::Root {
        username: control_user,
        password: control_pass,
    })
    .await
    .expect("signin");

    // Apply migration
    let migrations_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let migration = std::fs::read_to_string(migrations_dir.join("047_local_admin_auth.surql"))
        .expect("read migration");
    db.query(&migration).await.expect("apply migration");

    // Create test admin (SCHEMAFULL: id, version and updated_at are required).
    db.query("CREATE type::record('local_admin', 'race_admin') SET id = 'race_admin', username = 'race.admin', state = 'active', password_phc = NONE, credential_generation = 1, version = 1, created_at = time::now(), updated_at = time::now()")
        .await
        .expect("create admin");

    // Create two sessions. `local_admin_session` is SCHEMAFULL and has no
    // `created_at`/`revoked` fields; it requires `id`, `mode_epoch`,
    // `auth_time` and the `option<datetime>` `revoked_at`.
    db.query("CREATE type::record('local_admin_session', 'aaaa') SET id = 'aaaa', cookie_verifier = 'aaaa', admin_id = 'race_admin', credential_generation = 1, mode_epoch = 1, auth_time = time::now(), idle_expiry = time::now() + 1800s, absolute_expiry = time::now() + 86400s, revoked_at = NONE")
        .await
        .expect("create session 1");

    db.query("CREATE type::record('local_admin_session', 'bbbb') SET id = 'bbbb', cookie_verifier = 'bbbb', admin_id = 'race_admin', credential_generation = 1, mode_epoch = 1, auth_time = time::now(), idle_expiry = time::now() + 1800s, absolute_expiry = time::now() + 86400s, revoked_at = NONE")
        .await
        .expect("create session 2");

    // Concurrent revoke + resolve
    let db1 = Arc::new(db.clone());
    let db2 = Arc::new(db.clone());

    let handle1 = tokio::spawn(async move {
        db1.query("UPDATE local_admin_session SET revoked_at = time::now() WHERE cookie_verifier = 'aaaa'")
            .await
    });
    let handle2 = tokio::spawn(async move {
        db2.query(
            "SELECT * FROM local_admin_session WHERE cookie_verifier = 'bbbb' AND revoked_at IS NONE",
        )
        .await
    });

    let (r1, r2) = tokio::join!(handle1, handle2);
    let mut revoke = r1.expect("revoke task joined").expect("revoke query");
    assert!(
        revoke.take_errors().is_empty(),
        "revoke should not raise statement errors"
    );
    let mut resolve = r2.expect("resolve task joined").expect("resolve query");
    assert!(
        resolve.take_errors().is_empty(),
        "resolve should not raise statement errors"
    );

    // Typed postconditions: the independent session stays resolvable while the
    // other session's revocation is durably recorded.
    let resolved: Vec<serde_json::Value> = resolve.take(0).expect("resolve rows");
    assert_eq!(
        resolved.len(),
        1,
        "the independent session must remain resolvable during revoke"
    );
    assert_eq!(resolved[0]["cookie_verifier"], "bbbb");

    let revoked: Vec<serde_json::Value> = db
        .query("SELECT cookie_verifier, revoked_at FROM local_admin_session WHERE cookie_verifier = 'aaaa'")
        .await
        .expect("postcondition read")
        .take(0)
        .expect("revoked rows");
    assert_eq!(revoked.len(), 1, "the revoked session row must still exist");
    assert!(
        !revoked[0]["revoked_at"].is_null(),
        "the revoked session must record revoked_at"
    );

    // Cleanup
    let _ = db.query("REMOVE DATABASE test_session_race").await;
    let _ = db.query(format!("REMOVE NAMESPACE {namespace}")).await;
}
