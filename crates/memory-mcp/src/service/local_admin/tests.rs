//! E2E tests for local admin auth service against the real, migrated
//! in-memory SurrealDB control registry.

#[cfg(test)]
mod service_cases {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    use crate::http::registry::SurrealRegistryStore;
    use crate::service::local_admin::{
        AdminManagementService, AuthAttemptContext, ChallengeKind, LocalAdminAuthority,
        LocalAdminService, PasswordHasher, RequestContext,
    };

    async fn service() -> (AdminManagementService, LocalAdminService) {
        let namespace = format!("local_admin_test_{}", uuid::Uuid::new_v4().simple());
        let concrete = Arc::new(
            SurrealRegistryStore::connect_in_memory(&namespace, "registry")
                .await
                .expect("migrated Mem registry"),
        );
        let authority = LocalAdminAuthority::join_local_for_test(concrete, [1; 32], [2; 32])
            .await
            .expect("local policy");
        let management = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(
            authority,
            Arc::new(PasswordHasher::new().expect("supported KDF")),
        );
        (management, auth)
    }

    fn request() -> RequestContext {
        RequestContext {
            request_id: uuid::Uuid::new_v4(),
        }
    }

    fn attempt() -> AuthAttemptContext {
        AuthAttemptContext {
            request: request(),
            source: IpAddr::V4(Ipv4Addr::LOCALHOST),
        }
    }

    /// Decode the session cookie into the raw verifier `resolve` expects.
    fn cookie_verifier(cookie: &str) -> [u8; 32] {
        let value = cookie
            .strip_prefix("__Host-memory_mcp_admin=")
            .expect("session cookie prefix");
        hex::decode(value)
            .expect("hex cookie verifier")
            .try_into()
            .expect("32-byte cookie verifier")
    }

    #[tokio::test]
    async fn create_and_activate_admin() {
        let (mgmt, auth) = service().await;

        // Create admin
        let challenge = mgmt
            .create_admin("ops.one", &request())
            .await
            .expect("create admin");
        assert_eq!(challenge.issued.username, "ops.one");

        // Finish activation
        let auth_ctx = attempt();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .expect("finish challenge");

        // Login
        let login = auth
            .login(&auth_ctx, "ops.one", "SecureP@ssw0rd123".to_string())
            .await
            .expect("login");
        assert_eq!(login.principal.username, "ops.one");
        assert!(login.cookie.starts_with("__Host-memory_mcp_admin="));
    }

    #[tokio::test]
    async fn login_wrong_password_fails() {
        let (mgmt, auth) = service().await;

        // Create and activate
        let challenge = mgmt
            .create_admin("ops.one", &request())
            .await
            .expect("create admin");
        let auth_ctx = attempt();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .expect("finish challenge");

        // Login with wrong password
        let result = auth
            .login(&auth_ctx, "ops.one", "WrongPassword123".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn login_unknown_user_fails_with_dummy_hash() {
        let (_mgmt, auth) = service().await;

        // Login with unknown user
        let result = auth
            .login(&attempt(), "nonexistent", "SecureP@ssw0rd123".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_session_after_login() {
        let (mgmt, auth) = service().await;

        // Create, activate, login
        let challenge = mgmt
            .create_admin("ops.one", &request())
            .await
            .expect("create admin");
        let auth_ctx = attempt();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .expect("finish challenge");
        let login = auth
            .login(&auth_ctx, "ops.one", "SecureP@ssw0rd123".to_string())
            .await
            .expect("login");

        // Resolve session
        let verifier = cookie_verifier(&login.cookie);
        let principal = auth
            .resolve(&request(), &verifier)
            .await
            .expect("resolve session");
        assert_eq!(principal.username, "ops.one");
    }

    #[tokio::test]
    async fn logout_invalidates_session() {
        let (mgmt, auth) = service().await;

        // Create, activate, login
        let challenge = mgmt
            .create_admin("ops.one", &request())
            .await
            .expect("create admin");
        let auth_ctx = attempt();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .expect("finish challenge");
        let login = auth
            .login(&auth_ctx, "ops.one", "SecureP@ssw0rd123".to_string())
            .await
            .expect("login");

        // Logout
        auth.logout(&request(), &login.principal)
            .await
            .expect("logout");

        // Try to resolve session — should fail
        let verifier = cookie_verifier(&login.cookie);
        assert!(auth.resolve(&request(), &verifier).await.is_err());
    }

    #[tokio::test]
    async fn reauthenticate_rotates_session() {
        let (mgmt, auth) = service().await;

        // Create, activate, login
        let challenge = mgmt
            .create_admin("ops.one", &request())
            .await
            .expect("create admin");
        let auth_ctx = attempt();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .expect("finish challenge");
        let login = auth
            .login(&auth_ctx, "ops.one", "SecureP@ssw0rd123".to_string())
            .await
            .expect("login");

        // Reauthenticate
        let reauth = auth
            .reauthenticate(
                &attempt(),
                &login.principal,
                "SecureP@ssw0rd123".to_string(),
            )
            .await
            .expect("reauthenticate");
        assert_eq!(reauth.principal.username, "ops.one");
        // New cookie should be different
        assert_ne!(login.cookie, reauth.cookie);
    }

    #[tokio::test]
    async fn recover_admin_issues_reset_challenge() {
        let (mgmt, auth) = service().await;

        // Create and activate
        let challenge = mgmt
            .create_admin("ops.one", &request())
            .await
            .expect("create admin");
        let auth_ctx = attempt();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .expect("finish challenge");

        // Recover
        let reset = mgmt
            .recover_admin("ops.one", &request())
            .await
            .expect("recover");
        assert_eq!(reset.issued.username, "ops.one");

        // Finish reset with new password
        auth.finish_challenge(
            &auth_ctx,
            &reset.code,
            ChallengeKind::Reset,
            "NewSecureP@ss456".to_string(),
        )
        .await
        .expect("finish reset");

        // Login with new password
        let login = auth
            .login(&auth_ctx, "ops.one", "NewSecureP@ss456".to_string())
            .await
            .expect("login");
        assert_eq!(login.principal.username, "ops.one");
    }

    #[tokio::test]
    async fn local_admin_recovery_invalidates_old_credentials() {
        let (management, auth) = service().await;
        let invitation = management
            .create_admin("ops.one", &request())
            .await
            .expect("create");
        auth.finish_challenge(
            &attempt(),
            &invitation.code,
            ChallengeKind::Activate,
            "first correct password".into(),
        )
        .await
        .expect("activate");
        let login = auth
            .login(&attempt(), "ops.one", "first correct password".into())
            .await
            .expect("login");
        let reset = management
            .recover_admin("ops.one", &request())
            .await
            .expect("recover");
        let old_cookie = cookie_verifier(&login.cookie);
        assert!(auth.resolve(&request(), &old_cookie).await.is_err());
        assert!(
            auth.login(&attempt(), "ops.one", "first correct password".into())
                .await
                .is_err()
        );
        auth.finish_challenge(
            &attempt(),
            &reset.code,
            ChallengeKind::Reset,
            "second correct password".into(),
        )
        .await
        .expect("reset");
        assert!(
            auth.login(&attempt(), "ops.one", "second correct password".into())
                .await
                .is_ok()
        );
        assert!(auth.resolve(&request(), &old_cookie).await.is_err());
    }
}
