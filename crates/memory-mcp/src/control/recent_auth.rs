//! Recent-auth gate for destructive operations (Task 10.4, spec §5.3).
//!
//! Requires OIDC reauthentication within a configurable window
//! before allowing credential creation, identity linking, or
//! Account deletion.

use std::time::Duration;

use chrono::Utc;

use super::error::ApiError;
use super::session::ControlPlaneSession;

/// Default max age for recent auth: 10 minutes.
pub const DEFAULT_REAUTH_MAX_AGE: Duration = Duration::from_secs(600);

/// Require that the session was authenticated within `max_age`.
/// Returns `Err(ReauthRequired)` if the session is stale.
pub fn require_recent_auth(
    session: &ControlPlaneSession,
    max_age: Duration,
) -> Result<(), ApiError> {
    let elapsed = Utc::now() - session.auth_time;
    if elapsed > chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::seconds(600)) {
        return Err(ApiError::ReauthRequired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::Account;

    fn make_session(minutes_ago: i64) -> ControlPlaneSession {
        let account = Account {
            id: "acc1".to_string(),
            status: crate::http::registry::models::AccountStatus::Active,
            tenant_id: "t1".to_string(),
            created_at: Utc::now(),
        };
        let cfg = crate::http::config::HttpConfig::default_for_test();
        let mut session = ControlPlaneSession::new(&account, "raw-cookie", &cfg).unwrap();
        session.auth_time = Utc::now() - chrono::Duration::minutes(minutes_ago);
        session
    }

    #[test]
    fn recent_auth_within_window() {
        let session = make_session(5);
        assert!(require_recent_auth(&session, DEFAULT_REAUTH_MAX_AGE).is_ok());
    }

    #[test]
    fn recent_auth_outside_window() {
        let session = make_session(15);
        assert!(require_recent_auth(&session, DEFAULT_REAUTH_MAX_AGE).is_err());
    }

    #[test]
    fn recent_auth_custom_window() {
        let session = make_session(3);
        assert!(require_recent_auth(&session, Duration::from_secs(120)).is_err());
    }
}
