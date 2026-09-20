//! Security experiments (plan §5) for local admin authentication.
//!
//! Every experiment here runs against the **real durable store**
//! (`SurrealRegistryStore` over an in-memory SurrealDB engine), so the
//! assertions are backed by the same SQL transactions production uses. An
//! experiment is only listed when it asserts its own named invariant; cases
//! that cannot be expressed at this seam are mapped to the suite that does
//! cover them instead of being asserted against a test double.
//!
//! Where each §5 case is proven:
//!
//! | §5 case | Evidence |
//! |---|---|
//! | Same activation/reset verifier through independent handles | `exp1_same_verifier_double_submit` (durable) |
//! | KDF completes, pause before session insert; recover on a second handle | `http_local_admin.rs::client_mutation_follows_the_session_fence`, `reset_changes_the_password_and_fences_old_sessions` |
//! | Reset hash prepared, newer recovery commits | `exp3_recovery_invalidates_old_credentials` (generation, not interleaving) |
//! | Resolve/touch and recovery/logout, both commit orders | `exp4_revoke_after_login`; interleavings unproven |
//! | Client mutation paused after middleware, logout commits | `http_local_admin.rs::client_mutation_follows_the_session_fence` |
//! | Two rotations | `exp6_reauth_rotates_session`; interleavings unproven |
//! | Replica joins wrong mode or wrong local key fingerprints | `exp16_wrong_mode_policy_fingerprint_mismatch` (durable) |
//! | OIDC local→OIDC transition and legacy epoch-less session | `surreal_store.rs::find_session_rejects_legacy_and_stale_epoch_rows` |
//! | Rate two handles/restart/collision/window boundary | `exp9_rate_buckets_enforce_the_cap` (durable cap; restart persistence unproven) |
//! | Spoofed forwarding headers, missing peer, mapped IPv6 | `http_local_admin.rs::missing_peer_fails_closed`; `control::local_admin` peer tests |
//! | KDF queue/timeout/cancel/corrupt PHC | `password.rs` unit tests |
//! | Nonselected SQL statement error / audit insert error | `surreal_store/local_admin.rs::sql_fault_tests` (4 experiments); `surreal_store.rs::query_json_at_*` for the adapter-level propagation |
//! | DB unavailable during reserve/auth/success audit/failure audit | `sql_fault_tests::reservation_storage_error_fails_closed_without_admitting_the_attempt`; `sql_fault_tests::failure_audit_storage_error_is_sanitized_unavailable_not_a_rejection`; `control::local_admin::handlers::tests::spec_status_table_is_exhaustive` |
//! | Two admins issue at cap−1 | `http_local_admin.rs::active_key_cap_counts_only_live_keys` |
//! | Create same operation/body; different body; lost response | `http_local_admin.rs::client_lifecycle_uses_the_durable_store` |
//! | Key issue response lost and repeated | `insert_client_key` returns `AlreadyIssued`; `control-plane-ui` `secret_already_issued` tests |
//! | Provisioner restart | `http_crash_recovery.rs` (10 tests) |
//! | Coherent suspend/resume, stale CAS | `http_local_admin.rs::suspend_and_resume_follow_the_coherent_state_contract` |
//! | Warm key cache; revoke/expire | `http_local_admin.rs::revoking_an_issued_key_denies_with_and_without_a_warm_cache`, `expiry_at_the_boundary_is_rejected` |
//! | Origin/CSRF/content type/body/duplicate cookie/unknown fields | `http_local_admin.rs` (37 tests) |
//! | Cookie/bearer privilege separation, unmounted APIs | `bearer_keys_cannot_authenticate_local_admin_routes`, `unmatched_api_and_auth_paths_are_json_404_not_html` |
//! | Packaged UI/CLI over trusted TLS | `scripts/ci/local_admin_image.py --scenario all` (real browser) |

#[cfg(test)]
mod tests {
    use crate::http::registry::SurrealRegistryStore;
    use crate::service::local_admin::auth::{
        AdminManagementService, LocalAdminAuthority, LocalAdminService,
    };
    use crate::service::local_admin::contracts::{
        AttemptDecision, AttemptDomain, AttemptInput, AuthAttemptContext, ChallengeKind,
        LocalAdminError, RequestContext,
    };
    use crate::service::local_admin::password::PasswordHasher;
    use std::sync::Arc;

    fn make_request() -> RequestContext {
        RequestContext {
            request_id: uuid::Uuid::new_v4(),
        }
    }

    fn make_auth() -> AuthAttemptContext {
        AuthAttemptContext {
            request: make_request(),
            source: std::net::IpAddr::from([127, 0, 0, 1]),
        }
    }

    /// Decode a session cookie into the raw verifier `resolve` expects.
    fn cookie_verifier(cookie: &str) -> [u8; 32] {
        let value = cookie
            .strip_prefix("__Host-memory_mcp_admin=")
            .expect("session cookie prefix");
        hex::decode(value)
            .expect("hex cookie verifier")
            .try_into()
            .expect("32-byte cookie verifier")
    }

    /// A fresh migrated in-memory registry plus a joined local policy and the
    /// real KDF. Each experiment gets its own namespace so shared throttle
    /// counters and challenge rows cannot leak between cases.
    async fn setup() -> (Arc<LocalAdminAuthority>, Arc<PasswordHasher>) {
        let namespace = format!("local_admin_sec_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(
            SurrealRegistryStore::connect_in_memory(&namespace, "registry")
                .await
                .expect("migrated Mem registry"),
        );
        let authority = LocalAdminAuthority::join(store, [1u8; 32], [2u8; 32])
            .await
            .expect("join durable local policy");
        let hasher = Arc::new(PasswordHasher::new().expect("supported KDF"));
        (authority, hasher)
    }

    /// Create + activate an administrator and log in, returning the login.
    async fn activated_login(
        mgmt: &AdminManagementService,
        auth: &LocalAdminService,
        password: &str,
    ) -> crate::service::local_admin::contracts::AdminLogin {
        let challenge = mgmt
            .create_admin("ops.one", &make_request())
            .await
            .expect("create admin");
        auth.finish_challenge(
            &make_auth(),
            &challenge.code,
            ChallengeKind::Activate,
            password.to_owned(),
        )
        .await
        .expect("activate");
        auth.login(&make_auth(), "ops.one", password.to_owned())
            .await
            .expect("login")
    }

    #[tokio::test]
    async fn exp1_same_verifier_double_submit() {
        // Same activation verifier submitted twice must produce exactly one
        // consumption: the first finish wins, the replay is rejected.
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let challenge = mgmt
            .create_admin("ops.one", &make_request())
            .await
            .expect("create admin");
        let code = challenge.code.clone();

        let first = auth
            .finish_challenge(
                &make_auth(),
                &code,
                ChallengeKind::Activate,
                "SecureP@ssw0rd123".to_owned(),
            )
            .await;
        assert!(first.is_ok(), "the first consumption must succeed");

        let second = auth
            .finish_challenge(
                &make_auth(),
                &code,
                ChallengeKind::Activate,
                "AnotherP@ss456789".to_owned(),
            )
            .await;
        assert!(
            second.is_err(),
            "a consumed challenge verifier must not be usable again"
        );

        // The password the first call supplied is the one that now works.
        assert!(
            auth.login(&make_auth(), "ops.one", "SecureP@ssw0rd123".to_owned())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn exp2_wrong_kind_rejected() {
        // An activation challenge must not satisfy a reset.
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let challenge = mgmt
            .create_admin("ops.one", &make_request())
            .await
            .expect("create admin");

        let wrong_kind = auth
            .finish_challenge(
                &make_auth(),
                &challenge.code,
                ChallengeKind::Reset,
                "SecureP@ssw0rd123".to_owned(),
            )
            .await;
        assert!(wrong_kind.is_err(), "wrong-kind challenge must be rejected");

        // The correct kind still works, so the rejection did not consume it.
        assert!(
            auth.finish_challenge(
                &make_auth(),
                &challenge.code,
                ChallengeKind::Activate,
                "SecureP@ssw0rd123".to_owned(),
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn exp3_recovery_invalidates_old_credentials() {
        // Recovery must bump the credential generation: old sessions fail and
        // the old password stops working, while the reset code still works.
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let login = activated_login(&mgmt, &auth, "FirstP@ss1234567").await;
        assert!(
            auth.resolve(&make_request(), &cookie_verifier(&login.cookie))
                .await
                .is_ok(),
            "the fresh session must resolve"
        );

        let reset = mgmt
            .recover_admin("ops.one", &make_request())
            .await
            .expect("recover");

        assert!(
            auth.resolve(&make_request(), &cookie_verifier(&login.cookie))
                .await
                .is_err(),
            "recovery must invalidate existing sessions"
        );
        assert!(
            auth.login(&make_auth(), "ops.one", "FirstP@ss1234567".to_owned())
                .await
                .is_err(),
            "the old password must stop working"
        );

        auth.finish_challenge(
            &make_auth(),
            &reset.code,
            ChallengeKind::Reset,
            "SecondP@ss1234567".to_owned(),
        )
        .await
        .expect("reset");
        assert!(
            auth.login(&make_auth(), "ops.one", "SecondP@ss1234567".to_owned())
                .await
                .is_ok(),
            "the new password must work"
        );
    }

    #[tokio::test]
    async fn exp4_revoke_after_login() {
        // Logout must revoke exactly the presented session.
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let login = activated_login(&mgmt, &auth, "SecureP@ssw0rd123").await;
        let verifier = cookie_verifier(&login.cookie);
        auth.logout(&make_request(), &login.principal)
            .await
            .expect("logout");
        assert!(
            auth.resolve(&make_request(), &verifier).await.is_err(),
            "a logged-out session must not resolve"
        );
    }

    #[tokio::test]
    async fn exp5_wrong_password_does_not_leak_account_existence() {
        // Unknown user and wrong password must be indistinguishable.
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let challenge = mgmt
            .create_admin("ops.one", &make_request())
            .await
            .expect("create admin");
        auth.finish_challenge(
            &make_auth(),
            &challenge.code,
            ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_owned(),
        )
        .await
        .expect("activate");

        let unknown = auth
            .login(&make_auth(), "nobody.here", "SecureP@ssw0rd123".to_owned())
            .await
            .expect_err("unknown user must be rejected");
        let wrong = auth
            .login(&make_auth(), "ops.one", "WrongP@ssw0rd123".to_owned())
            .await
            .expect_err("wrong password must be rejected");

        // Both paths must raise the same public error, which is what the HTTP
        // mapper collapses onto a single `401 invalid_credentials`.
        assert!(
            matches!(unknown, LocalAdminError::InvalidCredentials),
            "unknown user must be invalid credentials: {unknown:?}"
        );
        assert!(
            matches!(wrong, LocalAdminError::InvalidCredentials),
            "wrong password must be invalid credentials: {wrong:?}"
        );
    }

    #[tokio::test]
    async fn exp6_reauth_rotates_session() {
        // Reauth must rotate the cookie and leave the old one dead.
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let login = activated_login(&mgmt, &auth, "SecureP@ssw0rd123").await;
        let old_verifier = cookie_verifier(&login.cookie);
        let rotated = auth
            .reauthenticate(
                &make_auth(),
                &login.principal,
                "SecureP@ssw0rd123".to_owned(),
            )
            .await
            .expect("reauth");

        assert_ne!(
            rotated.cookie, login.cookie,
            "reauth must rotate the cookie"
        );
        assert_eq!(
            rotated.principal.fence.admin_id, login.principal.fence.admin_id,
            "rotation keeps the same administrator"
        );
        assert!(
            auth.resolve(&make_request(), &cookie_verifier(&rotated.cookie))
                .await
                .is_ok(),
            "the rotated session must resolve"
        );
        assert!(
            auth.resolve(&make_request(), &old_verifier).await.is_err(),
            "the superseded session must not resolve"
        );
    }

    /// The credential cap is durable: the per-username bucket allows exactly
    /// five attempts per fixed window and then reports the reopen delay.
    #[tokio::test]
    async fn exp9_rate_buckets_enforce_the_cap() {
        let (authority, _hasher) = setup().await;
        let input = AttemptInput {
            domain: AttemptDomain::Credentials,
            username_bucket: Some(100),
            source_bucket: 200,
            policy: authority.policy().clone(),
            request: make_request(),
        };

        for attempt in 1..=5 {
            let decision = authority
                .store()
                .reserve_attempt(input.clone())
                .await
                .expect("reserve attempt");
            assert!(
                matches!(decision, AttemptDecision::Allowed),
                "attempt {attempt} of the 5-attempt cap must be allowed"
            );
        }

        match authority
            .store()
            .reserve_attempt(input.clone())
            .await
            .expect("reserve attempt")
        {
            AttemptDecision::Limited {
                retry_after_seconds,
            } => assert!(
                retry_after_seconds > 0,
                "a throttled attempt must report a positive reopen delay"
            ),
            AttemptDecision::Allowed => panic!("the 6th attempt must be throttled"),
        }

        // A different username bucket is unaffected: the cap is per identity,
        // not per source.
        let other = AttemptInput {
            username_bucket: Some(101),
            ..input
        };
        assert!(
            matches!(
                authority
                    .store()
                    .reserve_attempt(other)
                    .await
                    .expect("reserve attempt"),
                AttemptDecision::Allowed
            ),
            "a different username bucket must keep its own budget"
        );
    }

    /// The challenge domain is shared by inspect, activate and reset, and its
    /// cap is a source cap rather than a per-identity one.
    #[tokio::test]
    async fn exp9b_challenge_budget_is_shared_and_source_scoped() {
        let (authority, _hasher) = setup().await;
        let input = AttemptInput {
            domain: AttemptDomain::Challenge,
            username_bucket: None,
            source_bucket: 200,
            policy: authority.policy().clone(),
            request: make_request(),
        };

        for attempt in 1..=10 {
            assert!(
                matches!(
                    authority
                        .store()
                        .reserve_attempt(input.clone())
                        .await
                        .expect("reserve attempt"),
                    AttemptDecision::Allowed
                ),
                "challenge attempt {attempt} of the 10-attempt cap must be allowed"
            );
        }
        assert!(
            matches!(
                authority
                    .store()
                    .reserve_attempt(input)
                    .await
                    .expect("reserve attempt"),
                AttemptDecision::Limited { .. }
            ),
            "the 11th challenge attempt must be throttled"
        );
    }

    #[tokio::test]
    async fn exp13_password_policy_enforced() {
        use crate::service::local_admin::policy;

        assert!(policy::validate_password("short").is_err());
        assert!(policy::validate_password(&"a".repeat(15)).is_ok());
        assert!(policy::validate_password(&"a".repeat(128)).is_ok());
        assert!(policy::validate_password(&"a".repeat(129)).is_err());
        assert!(policy::validate_password("with\0null").is_err());
    }

    #[tokio::test]
    async fn exp14_username_normalization() {
        use crate::service::local_admin::policy;

        assert_eq!(
            policy::normalize_username("  Ops.One  ").unwrap(),
            "ops.one"
        );
        assert!(policy::normalize_username("ab").is_err());
        assert!(policy::normalize_username("a b").is_err());
        assert!(policy::normalize_username("-admin").is_err());
        assert!(policy::normalize_username(&"a".repeat(64)).is_ok());
        assert!(policy::normalize_username(&"a".repeat(65)).is_err());
    }

    /// Concurrent logins for one identity must each succeed and must never
    /// share a cookie.
    #[tokio::test]
    async fn exp15_concurrent_logins_same_user() {
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let challenge = mgmt
            .create_admin("ops.one", &make_request())
            .await
            .expect("create admin");
        auth.finish_challenge(
            &make_auth(),
            &challenge.code,
            ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_owned(),
        )
        .await
        .expect("activate");

        // Three, not five: the shared credential budget is five attempts per
        // username per window.
        let mut handles = Vec::new();
        for _ in 0..3 {
            let auth = auth.clone();
            handles.push(tokio::spawn(async move {
                auth.login(&make_auth(), "ops.one", "SecureP@ssw0rd123".to_owned())
                    .await
            }));
        }

        let mut logins = Vec::new();
        for handle in handles {
            logins.push(handle.await.expect("join").expect("admitted login"));
        }

        let cookies: Vec<_> = logins.iter().map(|login| login.cookie.clone()).collect();
        let unique: std::collections::HashSet<_> = cookies.iter().collect();
        assert_eq!(
            unique.len(),
            cookies.len(),
            "sessions must not share cookies"
        );

        // Every issued session must resolve independently.
        for cookie in &cookies {
            assert!(
                auth.resolve(&make_request(), &cookie_verifier(cookie))
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn exp16_wrong_mode_policy_fingerprint_mismatch() {
        // A replica joining with different key material must be refused.
        let namespace = format!("local_admin_sec_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(
            SurrealRegistryStore::connect_in_memory(&namespace, "registry")
                .await
                .expect("migrated Mem registry"),
        );
        assert!(
            LocalAdminAuthority::join(store.clone(), [1u8; 32], [2u8; 32])
                .await
                .is_ok()
        );
        assert!(
            LocalAdminAuthority::join(store, [3u8; 32], [4u8; 32])
                .await
                .is_err(),
            "a differing local key fingerprint must fail the join"
        );
    }

    #[tokio::test]
    async fn exp17_unknown_user_cannot_log_in() {
        let (authority, hasher) = setup().await;
        let auth = LocalAdminService::new(authority, hasher);

        let result = auth
            .login(&make_auth(), "nonexistent", "SecureP@ssw0rd123".to_owned())
            .await;
        assert!(result.is_err(), "an unknown username must not log in");
    }
}
