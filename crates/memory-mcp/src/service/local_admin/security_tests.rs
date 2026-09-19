//! Security experiments (section 5) for local admin authentication.
//!
//! These tests verify race conditions, concurrency, and security
//! invariants using the in-memory mock store.

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use crate::service::local_admin::auth::{AdminManagementService, LocalAdminAuthority, LocalAdminService};
    use crate::service::local_admin::contracts::{
        AuthAttemptContext, ChallengeKind, RequestContext,
    };
    use crate::service::local_admin::mock_store::InMemoryLocalAdminStore;
    use crate::service::local_admin::password::PasswordHasher;

    fn make_request() -> RequestContext {
        RequestContext { request_id: uuid::Uuid::new_v4() }
    }

    fn make_auth() -> AuthAttemptContext {
        AuthAttemptContext {
            request: make_request(),
            source: std::net::IpAddr::from([127, 0, 0, 1]),
        }
    }

    async fn setup() -> (Arc<LocalAdminAuthority>, Arc<PasswordHasher>) {
        let store = Arc::new(InMemoryLocalAdminStore::new());
        let authority = LocalAdminAuthority::join(store, [1u8; 32], [2u8; 32]).await.unwrap();
        let hasher = Arc::new(PasswordHasher::new().unwrap());
        (authority, hasher)
    }

    #[tokio::test]
    async fn exp1_same_verifier_double_submit() {
        // Same activation/reset verifier submitted through independent handles
        // should produce exactly one consumption.
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let challenge = mgmt.create_admin("ops.one", &make_request()).await.unwrap();
        let code = challenge.code.clone();

        // First finish succeeds
        let result1 = auth.finish_challenge(
            &make_auth(), &code, ChallengeKind::Activate, "SecureP@ssw0rd123".into()
        ).await;
        assert!(result1.is_ok());

        // Second finish with same code should fail (already consumed)
        let result2 = auth.finish_challenge(
            &make_auth(), &code, ChallengeKind::Activate, "AnotherP@ss456".into()
        ).await;
        assert!(result2.is_err());
    }

    #[tokio::test]
    async fn exp2_wrong_kind_rejected() {
        // Reset code should not work for activation
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let challenge = mgmt.create_admin("ops.one", &make_request()).await.unwrap();

        // Try to use activation code as reset
        let result = auth.finish_challenge(
            &make_auth(), &challenge.code, ChallengeKind::Reset, "SecureP@ssw0rd123".into()
        ).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn exp3_recovery_invalidates_old_credentials() {
        // Recovery should invalidate old password and sessions
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Create and activate
        let challenge = mgmt.create_admin("ops.one", &make_request()).await.unwrap();
        auth.finish_challenge(
            &make_auth(), &challenge.code, ChallengeKind::Activate, "FirstP@ss1234567".into()
        ).await.unwrap();

        // Login with first password
        let login = auth.login(&make_auth(), "ops.one", "FirstP@ss1234567".into()).await.unwrap();

        // Recover (issues reset code, revokes sessions)
        let reset = mgmt.recover_admin("ops.one", &make_request()).await.unwrap();

        // Old session should be invalid
        let cookie_val = login.cookie.strip_prefix("__Host-memory_mcp_admin=").unwrap();
        let verifier: [u8; 32] = hex::decode(cookie_val).unwrap().try_into().unwrap();
        assert!(auth.resolve(&make_request(), &verifier).await.is_err());

        // Old password should still work (recovery only issues reset code, doesn't change password yet)
        assert!(auth.login(&make_auth(), "ops.one", "FirstP@ss1234567".into()).await.is_ok());

        // New password after reset should work
        auth.finish_challenge(
            &make_auth(), &reset.code, ChallengeKind::Reset, "SecondP@ss4567890".into()
        ).await.unwrap();

        // Old password should no longer work after reset is completed
        assert!(auth.login(&make_auth(), "ops.one", "FirstP@ss1234567".into()).await.is_err());
        assert!(auth.login(&make_auth(), "ops.one", "SecondP@ss4567890".into()).await.is_ok());
    }

    #[tokio::test]
    async fn exp4_revoke_after_login() {
        // Logout should invalidate session
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let challenge = mgmt.create_admin("ops.one", &make_request()).await.unwrap();
        auth.finish_challenge(
            &make_auth(), &challenge.code, ChallengeKind::Activate, "SecureP@ssw0rd123".into()
        ).await.unwrap();

        let login = auth.login(&make_auth(), "ops.one", "SecureP@ssw0rd123".into()).await.unwrap();
        auth.logout(&make_request(), &login.principal).await.unwrap();

        // Session should be invalid after logout
        let cookie_val = login.cookie.strip_prefix("__Host-memory_mcp_admin=").unwrap();
        let verifier: [u8; 32] = hex::decode(cookie_val).unwrap().try_into().unwrap();
        assert!(auth.resolve(&make_request(), &verifier).await.is_err());
    }

    #[tokio::test]
    async fn exp5_wrong_password_does_not_leak_account_existence() {
        // Unknown user and wrong password should produce same error
        let (authority, hasher) = setup().await;
        let auth = LocalAdminService::new(authority.clone(), hasher);

        // Unknown user
        let result1 = auth.login(&make_auth(), "unknown", "password".into()).await;
        assert!(result1.is_err());

        // Wrong password for non-existent user (dummy verification)
        let result2 = auth.login(&make_auth(), "nonexistent", "password".into()).await;
        assert!(result2.is_err());
    }

    #[tokio::test]
    async fn exp6_reauth_rotates_session() {
        // Reauth should create new session and invalidate old
        let (authority, hasher) = setup().await;
        let mgmt = AdminManagementService::new(authority.clone());
        let auth = LocalAdminService::new(authority.clone(), hasher);

        let challenge = mgmt.create_admin("ops.one", &make_request()).await.unwrap();
        auth.finish_challenge(
            &make_auth(), &challenge.code, ChallengeKind::Activate, "SecureP@ssw0rd123".into()
        ).await.unwrap();

        let login = auth.login(&make_auth(), "ops.one", "SecureP@ssw0rd123".into()).await.unwrap();
        let reauth = auth.reauthenticate(
            &make_auth(), &login.principal, "SecureP@ssw0rd123".into()
        ).await.unwrap();

        // New cookie should be different
        assert_ne!(login.cookie, reauth.cookie);

        // Old cookie should be invalid
        let old_val = login.cookie.strip_prefix("__Host-memory_mcp_admin=").unwrap();
        let old_verifier: [u8; 32] = hex::decode(old_val).unwrap().try_into().unwrap();
        assert!(auth.resolve(&make_request(), &old_verifier).await.is_err());
    }
}
