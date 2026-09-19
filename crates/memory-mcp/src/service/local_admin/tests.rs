//! E2E tests for local admin auth service using the in-memory mock store.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::service::local_admin::auth::{
        AdminManagementService, LocalAdminAuthority, LocalAdminService,
    };
    use crate::service::local_admin::contracts::{
        AuthAttemptContext, BrowserAuthMode, RequestContext,
    };
    use crate::service::local_admin::mock_store::InMemoryLocalAdminStore;
    use crate::service::local_admin::password::PasswordHasher;

    fn make_request_context() -> RequestContext {
        RequestContext {
            request_id: uuid::Uuid::new_v4(),
        }
    }

    fn make_auth_context() -> AuthAttemptContext {
        AuthAttemptContext {
            request: make_request_context(),
            source: std::net::IpAddr::from([127, 0, 0, 1]),
        }
    }

    fn session_key() -> [u8; 32] {
        [0x01; 32]
    }

    fn csrf_key() -> [u8; 32] {
        [0x02; 32]
    }

    async fn setup() -> (Arc<LocalAdminAuthority>, Arc<PasswordHasher>) {
        let store = Arc::new(InMemoryLocalAdminStore::new());
        let authority = LocalAdminAuthority::join(store, session_key(), csrf_key())
            .await
            .expect("join policy");
        let hasher = Arc::new(PasswordHasher::new().expect("init hasher"));
        (authority, hasher)
    }

    #[tokio::test]
    async fn create_and_activate_admin() {
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Create admin
        let ctx = make_request_context();
        let challenge = mgmt
            .create_admin("ops.one", &ctx)
            .await
            .expect("create admin");
        assert_eq!(challenge.issued.username, "ops.one");

        // Finish activation
        let auth_ctx = make_auth_context();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            crate::service::local_admin::contracts::ChallengeKind::Activate,
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
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Create and activate
        let ctx = make_request_context();
        let challenge = mgmt.create_admin("ops.one", &ctx).await.unwrap();
        let auth_ctx = make_auth_context();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            crate::service::local_admin::contracts::ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .unwrap();

        // Login with wrong password
        let result = auth
            .login(&auth_ctx, "ops.one", "WrongPassword123".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn login_unknown_user_fails_with_dummy_hash() {
        let (authority, hasher) = setup().await;
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Login with unknown user
        let auth_ctx = make_auth_context();
        let result = auth
            .login(&auth_ctx, "nonexistent", "SecureP@ssw0rd123".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_session_after_login() {
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Create, activate, login
        let ctx = make_request_context();
        let challenge = mgmt.create_admin("ops.one", &ctx).await.unwrap();
        let auth_ctx = make_auth_context();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            crate::service::local_admin::contracts::ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .unwrap();
        let login = auth
            .login(&auth_ctx, "ops.one", "SecureP@ssw0rd123".to_string())
            .await
            .unwrap();

        // Parse cookie verifier
        let cookie_val = login
            .cookie
            .strip_prefix("__Host-memory_mcp_admin=")
            .unwrap();
        let verifier: [u8; 32] = hex::decode(cookie_val).unwrap().try_into().unwrap();

        // Resolve session
        let principal = auth
            .resolve(&make_request_context(), &verifier)
            .await
            .unwrap();
        assert_eq!(principal.username, "ops.one");
    }

    #[tokio::test]
    async fn logout_invalidates_session() {
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Create, activate, login
        let ctx = make_request_context();
        let challenge = mgmt.create_admin("ops.one", &ctx).await.unwrap();
        let auth_ctx = make_auth_context();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            crate::service::local_admin::contracts::ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .unwrap();
        let login = auth
            .login(&auth_ctx, "ops.one", "SecureP@ssw0rd123".to_string())
            .await
            .unwrap();

        // Logout
        auth.logout(&make_request_context(), &login.principal)
            .await
            .unwrap();

        // Try to resolve session — should fail
        let cookie_val = login
            .cookie
            .strip_prefix("__Host-memory_mcp_admin=")
            .unwrap();
        let verifier: [u8; 32] = hex::decode(cookie_val).unwrap().try_into().unwrap();
        let result = auth.resolve(&make_request_context(), &verifier).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn reauthenticate_rotates_session() {
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Create, activate, login
        let ctx = make_request_context();
        let challenge = mgmt.create_admin("ops.one", &ctx).await.unwrap();
        let auth_ctx = make_auth_context();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            crate::service::local_admin::contracts::ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .unwrap();
        let login = auth
            .login(&auth_ctx, "ops.one", "SecureP@ssw0rd123".to_string())
            .await
            .unwrap();

        // Reauthenticate
        let reauth = auth
            .reauthenticate(&auth_ctx, &login.principal, "SecureP@ssw0rd123".to_string())
            .await
            .unwrap();
        assert_eq!(reauth.principal.username, "ops.one");
        // New cookie should be different
        assert_ne!(login.cookie, reauth.cookie);
    }

    #[tokio::test]
    async fn recover_admin_issues_reset_challenge() {
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Create and activate
        let ctx = make_request_context();
        let challenge = mgmt.create_admin("ops.one", &ctx).await.unwrap();
        let auth_ctx = make_auth_context();
        auth.finish_challenge(
            &auth_ctx,
            &challenge.code,
            crate::service::local_admin::contracts::ChallengeKind::Activate,
            "SecureP@ssw0rd123".to_string(),
        )
        .await
        .unwrap();

        // Recover
        let reset = mgmt.recover_admin("ops.one", &ctx).await.unwrap();
        assert_eq!(reset.issued.username, "ops.one");

        // Finish reset with new password
        auth.finish_challenge(
            &auth_ctx,
            &reset.code,
            crate::service::local_admin::contracts::ChallengeKind::Reset,
            "NewSecureP@ss456".to_string(),
        )
        .await
        .unwrap();

        // Login with new password
        let login = auth
            .login(&auth_ctx, "ops.one", "NewSecureP@ss456".to_string())
            .await
            .unwrap();
        assert_eq!(login.principal.username, "ops.one");
    }
}
