//! Remote replica race tests (plan §6, Task 10).
//!
//! These are **inline adapter tests**: they live inside the crate so a run can
//! reach the private local-admin contracts, and so the documented command
//!
//! ```text
//! cargo test -p memory_mcp --lib --features control-plane,test-fixtures \
//!   --locked local_admin_remote_replica_races -- --ignored
//! ```
//!
//! actually selects them. An explicitly selected test **fails** when its
//! environment is missing — it never skips and never passes vacuously.
//!
//! They require an isolated SurrealDB 3.3.5 or 4.0.1 server and read
//! `LOCAL_ADMIN_TEST_CONTROL_URL`, `LOCAL_ADMIN_TEST_CONTROL_USERNAME` and
//! `LOCAL_ADMIN_TEST_CONTROL_PASSWORD`. The URL must point to loopback. Every
//! run uses a fresh random namespace and database, so a production target cannot
//! be touched by accident.
//!
//! The `Mem` engine cannot demonstrate replica behaviour: two handles over one
//! in-process engine share the same lock. These tests therefore use **two
//! independent remote connections**.

use std::sync::Arc;

use crate::config::SurrealTargetConfig;
use crate::service::local_admin::auth::AdminManagementService;
use crate::service::local_admin::contracts::{
    AdminPrincipal, AttemptDecision, AttemptDomain, AttemptInput, AuthAttemptContext,
    LocalAdminError, LocalAdminStore, RequestContext, SessionRotate,
};

use super::SurrealRegistryStore;

/// Read the three remote-test variables, failing (never skipping) when any is
/// absent.
fn remote_target() -> SurrealTargetConfig {
    let url = std::env::var("LOCAL_ADMIN_TEST_CONTROL_URL")
        .expect("LOCAL_ADMIN_TEST_CONTROL_URL must be set for this test");
    assert!(
        url.starts_with("ws://127.0.0.1:")
            || url.starts_with("ws://localhost:")
            || url.starts_with("ws://[::1]:"),
        "remote compatibility tests only allow a loopback URL"
    );
    let username = std::env::var("LOCAL_ADMIN_TEST_CONTROL_USERNAME")
        .expect("LOCAL_ADMIN_TEST_CONTROL_USERNAME must be set for this test");
    let password = std::env::var("LOCAL_ADMIN_TEST_CONTROL_PASSWORD")
        .expect("LOCAL_ADMIN_TEST_CONTROL_PASSWORD must be set for this test");
    let namespace = format!("local_admin_race_{}", uuid::Uuid::new_v4().simple());
    SurrealTargetConfig {
        url,
        username,
        password,
        database: format!("race_{}", uuid::Uuid::new_v4().simple()),
        namespace,
    }
}

/// Connects and applies the role catalog through the supported maintenance path.
async fn connect_maintained(target: &SurrealTargetConfig) -> SurrealRegistryStore {
    SurrealRegistryStore::connect_with_v4_maintenance(
        target,
        true,
        crate::storage::v4_maintenance::RecoveryAuthorization::None,
    )
    .await
    .expect("connect and maintain isolated remote registry")
}

/// Two independent connections to the same isolated remote registry.
async fn two_replicas(
    target: &SurrealTargetConfig,
) -> (Arc<SurrealRegistryStore>, Arc<SurrealRegistryStore>) {
    let first = Arc::new(connect_maintained(target).await);
    let second = Arc::new(connect_maintained(target).await);
    (first, second)
}

fn request() -> RequestContext {
    RequestContext {
        request_id: uuid::Uuid::new_v4(),
    }
}

async fn activated_remote_admin() -> (
    Arc<SurrealRegistryStore>,
    Arc<crate::service::local_admin::auth::LocalAdminAuthority>,
    AdminPrincipal,
) {
    let target = remote_target();
    let store = Arc::new(connect_maintained(&target).await);
    let authority = crate::service::local_admin::auth::LocalAdminAuthority::join_local_for_test(
        store.clone(),
        [7u8; 32],
        [8u8; 32],
    )
    .await
    .expect("join local policy");
    let management = AdminManagementService::new(authority.clone());
    let hasher = Arc::new(
        crate::service::local_admin::password::PasswordHasher::new().expect("supported KDF"),
    );
    let auth = crate::service::local_admin::auth::LocalAdminService::new(authority.clone(), hasher);
    let challenge = management
        .create_admin("return.contract", &request())
        .await
        .expect("create administrator");
    auth.finish_challenge(
        &AuthAttemptContext {
            request: request(),
            source: std::net::IpAddr::from([127, 0, 0, 1]),
        },
        &challenge.code,
        crate::service::local_admin::contracts::ChallengeKind::Activate,
        "a sufficiently long passphrase".to_owned(),
    )
    .await
    .expect("activate administrator");
    let login = auth
        .login(
            &AuthAttemptContext {
                request: request(),
                source: std::net::IpAddr::from([127, 0, 0, 1]),
            },
            "return.contract",
            "a sufficiently long passphrase".to_owned(),
        )
        .await
        .expect("login");

    (store, authority, login.principal)
}

/// Two replicas racing to create the same canonical administrator.
///
/// The unique `username` index is the arbiter: exactly one insert commits and
/// the loser fails with a typed error, never a partial write. Both replicas
/// then observe the same single row.
#[tokio::test]
#[ignore = "requires isolated loopback SurrealDB 3.3.5 or 4.0.1"]
async fn local_admin_remote_replica_races() {
    let target = remote_target();
    let (first, second) = two_replicas(&target).await;

    // Both replicas reconcile the singleton policy: the first creates it, the
    // second must observe an identical epoch and the same method set rather
    // than creating a second row.
    let authority_one =
        crate::service::local_admin::auth::LocalAdminAuthority::join_local_for_test(
            first.clone(),
            [7u8; 32],
            [8u8; 32],
        )
        .await
        .expect("replica one joins the local policy");
    let authority_two =
        crate::service::local_admin::auth::LocalAdminAuthority::join_local_for_test(
            second.clone(),
            [7u8; 32],
            [8u8; 32],
        )
        .await
        .expect("replica two joins the same local policy");
    assert_eq!(
        authority_one.policy().epoch,
        authority_two.policy().epoch,
        "both replicas must observe the same policy epoch"
    );

    // Race the first activation challenge for one canonical username.
    let one = AdminManagementService::new(authority_one.clone());
    let two = AdminManagementService::new(authority_two.clone());
    let left_request = request();
    let right_request = request();
    let (left, right) = tokio::join!(
        one.create_admin("race.one", &left_request),
        two.create_admin("race.one", &right_request)
    );
    let (winner, loser) = match (left, right) {
        (Ok(winner), Err(loser)) | (Err(loser), Ok(winner)) => (winner, loser),
        other => panic!("exactly one replica may create the administrator: {other:?}"),
    };
    assert!(
        !matches!(loser, LocalAdminError::Unavailable),
        "a lost race is a conflict, not an outage: {loser:?}"
    );

    // Both replicas read the same single administrator row through the
    // production query path.
    let rows = first
        .admin_query(
            "SELECT VALUE record::id(id) FROM local_admin WHERE username = $username;",
            Some(serde_json::json!({ "username": "race.one" })),
        )
        .await
        .expect("replica one reads the admin");
    let rows_other = second
        .admin_query(
            "SELECT VALUE record::id(id) FROM local_admin WHERE username = $username;",
            Some(serde_json::json!({ "username": "race.one" })),
        )
        .await
        .expect("replica two reads the admin");
    assert_eq!(rows.len(), 1, "exactly one admin row exists");
    assert_eq!(rows, rows_other, "both replicas observe the same row");
    assert_eq!(
        rows[0].as_str(),
        Some(winner.issued.admin_id.as_str()),
        "the persisted row is the winner's"
    );
}

/// Two replicas racing a session revocation against a session resolution.
///
/// The revocation must be durable and the untouched session must stay
/// resolvable; neither transaction may raise a statement error.
#[tokio::test]
#[ignore = "requires isolated loopback SurrealDB 3.3.5 or 4.0.1"]
async fn local_admin_session_revocation_race() {
    let target = remote_target();
    let (first, second) = two_replicas(&target).await;

    let authority_one =
        crate::service::local_admin::auth::LocalAdminAuthority::join_local_for_test(
            first.clone(),
            [7u8; 32],
            [8u8; 32],
        )
        .await
        .expect("replica one joins the local policy");
    let authority_two =
        crate::service::local_admin::auth::LocalAdminAuthority::join_local_for_test(
            second.clone(),
            [7u8; 32],
            [8u8; 32],
        )
        .await
        .expect("replica two joins the same local policy");

    // Provision an activated administrator on replica one, then log in twice
    // so two independent sessions exist.
    let management = AdminManagementService::new(authority_one.clone());
    let hasher = Arc::new(
        crate::service::local_admin::password::PasswordHasher::new().expect("supported KDF"),
    );
    let auth_one = crate::service::local_admin::auth::LocalAdminService::new(
        authority_one.clone(),
        hasher.clone(),
    );
    let challenge = management
        .create_admin("race.admin", &request())
        .await
        .expect("create admin");
    let attempt = |source: [u8; 4]| crate::service::local_admin::contracts::AuthAttemptContext {
        request: request(),
        source: std::net::IpAddr::from(source),
    };
    auth_one
        .finish_challenge(
            &attempt([127, 0, 0, 1]),
            &challenge.code,
            crate::service::local_admin::contracts::ChallengeKind::Activate,
            "a sufficiently long passphrase".to_owned(),
        )
        .await
        .expect("activate");

    // Log in through *different* replicas so the two sessions are created by
    // independent connections.
    let auth_two = crate::service::local_admin::auth::LocalAdminService::new(authority_two, hasher);
    let first_login = auth_one
        .login(
            &attempt([127, 0, 0, 1]),
            "race.admin",
            "a sufficiently long passphrase".to_owned(),
        )
        .await
        .expect("first login");
    let second_login = auth_two
        .login(
            &attempt([127, 0, 0, 2]),
            "race.admin",
            "a sufficiently long passphrase".to_owned(),
        )
        .await
        .expect("second login");

    // Race: replica one logs out its session while replica two resolves its
    // own, independent session.
    let logout_request = request();
    let resolve_request = request();
    let second_verifier = cookie_verifier(&second_login.cookie_value);
    let first_verifier = cookie_verifier(&first_login.cookie_value);
    let (logout, resolved) = tokio::join!(
        auth_one.logout(&logout_request, &first_login.principal),
        auth_two.resolve(&resolve_request, &second_verifier)
    );
    logout.expect("logout commits");
    // Exact postcondition, not "it did not error": the concurrent read must
    // return the second session's own principal.
    let resolved = resolved.expect("resolving an independent session during a logout");
    assert_eq!(
        resolved.fence.session_id, second_login.principal.fence.session_id,
        "the resolved session is the one that was presented"
    );
    assert_eq!(resolved.username, "race.admin");

    // The revoked session no longer resolves; the independent one still does.
    let revoked_request = request();
    let revoked = auth_one.resolve(&revoked_request, &first_verifier).await;
    match revoked {
        Err(LocalAdminError::Unauthenticated) => {}
        other => panic!("a revoked session must be rejected as unauthenticated: {other:?}"),
    }
    let survivor_request = request();
    let survivor = auth_two
        .resolve(&survivor_request, &second_verifier)
        .await
        .expect("the independent session must survive the other session's logout");
    assert_eq!(
        survivor.fence.admin_id, first_login.principal.fence.admin_id,
        "the survivor belongs to the same administrator as the revoked session"
    );
}

/// A rate window is durable state, not process state: after a connection
/// goes away (a restart), a fresh connection to the same registry still
/// sees the spent budget instead of silently resetting it. Plan §5 lists
/// "restart" alongside the two-handle rate cases.
#[tokio::test]
#[ignore = "requires isolated loopback SurrealDB 3.3.5 or 4.0.1"]
async fn local_admin_rate_window_survives_a_store_reconnect() {
    use crate::service::local_admin::contracts::{
        AttemptDecision, AttemptDomain, AttemptInput, LocalAdminStore,
    };

    let target = remote_target();
    let before = Arc::new(connect_maintained(&target).await);
    let authority = crate::service::local_admin::auth::LocalAdminAuthority::join_local_for_test(
        before.clone(),
        [7u8; 32],
        [8u8; 32],
    )
    .await
    .expect("join the local policy");
    let attempt = || AttemptInput {
        domain: AttemptDomain::Challenge,
        username_bucket: None,
        source_bucket: 0,
        policy: authority.policy().clone(),
        request: request(),
    };

    // Spend the challenge domain's whole per-source budget (10 per window).
    for _ in 0..10 {
        let decision = before.reserve_attempt(attempt()).await.expect("reserve");
        assert!(
            matches!(decision, AttemptDecision::Allowed),
            "the budget admits exactly ten attempts: {decision:?}"
        );
    }

    // "Restart": the old handle goes away mid-window, a fresh connection
    // opens against the same registry.
    drop(before);
    let after = Arc::new(connect_maintained(&target).await);
    match after.reserve_attempt(attempt()).await.expect("reserve") {
        AttemptDecision::Limited {
            retry_after_seconds,
        } => {
            assert!(retry_after_seconds > 0, "the window states when it reopens");
        }
        other => panic!("the spent budget must survive the restart: {other:?}"),
    }
}

/// Decode the bare-hex session verifier (`AdminLogin::cookie_value`) into
/// the 32-byte value the service resolves with (the same shape the HTTP
/// layer parses out of either cookie name).
fn cookie_verifier(cookie_value: &str) -> [u8; 32] {
    hex::decode(cookie_value)
        .expect("hex cookie verifier")
        .try_into()
        .expect("32-byte cookie verifier")
}

async fn count_rows(store: &SurrealRegistryStore, table: &str) -> Vec<serde_json::Value> {
    store
        .admin_query(&format!("SELECT * FROM {table};"), None)
        .await
        .unwrap_or_else(|error| panic!("read {table}: {error}"))
}

#[tokio::test]
#[ignore = "requires isolated loopback SurrealDB 3.3.5 or 4.0.1"]
async fn local_admin_remote_resolve_session_returns_and_persists_one_session() {
    let (store, authority, principal) = activated_remote_admin().await;
    let verifier = cookie_verifier(&principal.fence.session_id);

    let resolved = store
        .resolve_session(&verifier, authority.policy())
        .await
        .expect("resolve the live session");

    assert_eq!(resolved.fence.admin_id, principal.fence.admin_id);
    assert_eq!(resolved.username, "return.contract");
    assert_eq!(resolved.fence.session_id, principal.fence.session_id);
    let sessions = count_rows(&store, "local_admin_session").await;
    assert_eq!(
        sessions.len(),
        1,
        "resolution must not create another session"
    );
    assert_eq!(sessions[0]["cookie_verifier"], principal.fence.session_id);
}

#[tokio::test]
#[ignore = "requires isolated loopback SurrealDB 3.3.5 or 4.0.1"]
async fn local_admin_remote_rotate_session_commits_exactly_one_replacement() {
    let (store, authority, principal) = activated_remote_admin().await;
    let credential = store
        .credential("return.contract", authority.policy())
        .await
        .expect("read the active credential")
        .expect("credential exists");
    let verifier = [9u8; 32];
    let rotated = store
        .rotate_session(SessionRotate {
            fence: principal.fence.clone(),
            credential,
            cookie_verifier: verifier,
            request: request(),
        })
        .await
        .expect("rotate the session");

    let replacement = hex::encode(verifier);
    assert_eq!(rotated.fence.admin_id, principal.fence.admin_id);
    assert_eq!(rotated.fence.session_id, replacement);
    assert_eq!(rotated.username, "return.contract");
    assert_eq!(rotated.absolute_expiry, principal.absolute_expiry);

    let sessions = count_rows(&store, "local_admin_session").await;
    assert_eq!(
        sessions.len(),
        2,
        "rotation keeps one revoked and one new row"
    );
    assert_eq!(
        sessions
            .iter()
            .filter(|row| row["revoked_at"].is_null())
            .count(),
        1,
        "exactly one session remains active"
    );
    assert!(sessions.iter().any(|row| {
        row["cookie_verifier"] == principal.fence.session_id && !row["revoked_at"].is_null()
    }));
    assert!(
        sessions
            .iter()
            .any(|row| { row["cookie_verifier"] == replacement && row["revoked_at"].is_null() })
    );
    let audits = store
        .admin_query(
            "SELECT * FROM local_admin_audit WHERE action = 'reauth';",
            None,
        )
        .await
        .expect("read rotation audit rows");
    assert_eq!(audits.len(), 1, "rotation writes one audit record");
}

#[tokio::test]
#[ignore = "requires isolated loopback SurrealDB 3.3.5 or 4.0.1"]
async fn local_admin_remote_reserve_attempt_keeps_one_saturated_bucket() {
    let target = remote_target();
    let store = Arc::new(connect_maintained(&target).await);
    let authority = crate::service::local_admin::auth::LocalAdminAuthority::join_local_for_test(
        store.clone(),
        [7u8; 32],
        [8u8; 32],
    )
    .await
    .expect("join local policy");
    let attempt = || AttemptInput {
        domain: AttemptDomain::Challenge,
        username_bucket: None,
        source_bucket: 17,
        policy: authority.policy().clone(),
        request: request(),
    };

    for attempt_number in 1..=10 {
        assert!(
            matches!(
                store
                    .reserve_attempt(attempt())
                    .await
                    .expect("reserve attempt"),
                AttemptDecision::Allowed
            ),
            "attempt {attempt_number} is within the challenge budget"
        );
    }
    assert!(matches!(
        store
            .reserve_attempt(attempt())
            .await
            .expect("reserve saturated attempt"),
        AttemptDecision::Limited {
            retry_after_seconds: 1..=900
        }
    ));

    let buckets = store
        .admin_query(
            "SELECT * FROM local_admin_rate_bucket WHERE action = 'challenge';",
            None,
        )
        .await
        .expect("read persisted challenge buckets");
    assert_eq!(buckets.len(), 1, "one source slot has one persisted bucket");
    assert_eq!(buckets[0]["denied_count"], 11);
}
