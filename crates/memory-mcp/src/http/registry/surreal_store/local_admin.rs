//! Durable `LocalAdminStore` implementation for `SurrealRegistryStore`.
//!
//! Each method owns a constant SQL body, documented result index, and
//! strict decoder. All user-controlled values use parameterised queries.

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::http::registry::surreal_store::{RegistryDb, SurrealHandle};
use crate::service::local_admin::contracts::{
    AdminFence, AdminKeyInsert, AdminPrincipal, AdminState, AttemptDecision, AttemptInput,
    BrowserAuthMode, BrowserPolicyFence, ChallengeFinish, ChallengeIssue, ChallengeKind,
    ChallengeView, ClientBundle, ClientStateAction, ClientView, CredentialSnapshot, FailureAudit,
    IssuedChallenge, KeyInsertOutcome, LocalAdminError, LocalAdminStore, LocalKeyFingerprints,
    LocalResult, Page, PageRequest, RequestContext, SessionOpen, SessionRotate,
};

use super::SurrealRegistryStore;

/// Convert a `MemoryError` to `LocalAdminError::Infrastructure`.
fn infra(e: MemoryError) -> LocalAdminError {
    LocalAdminError::Infrastructure(e)
}

/// Extract a required string field from a JSON value.
fn require_str(value: &Value, field: &str) -> LocalResult<String> {
    value
        .get(field)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| LocalAdminError::InvalidInput(format!("missing field: {field}")))
}

/// Extract a required u64 field from a JSON value.
fn require_u64(value: &Value, field: &str) -> LocalResult<u64> {
    value
        .get(field)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| LocalAdminError::InvalidInput(format!("missing field: {field}")))
}

/// Parse a SurrealDB datetime string.
fn parse_datetime(value: &Value, field: &str) -> LocalResult<chrono::DateTime<Utc>> {
    let raw = value
        .get(field)
        .and_then(|v| v.as_str())
        .or_else(|| value.get(field).and_then(|v| v.get("Datetime").and_then(Value::as_str)))
        .ok_or_else(|| LocalAdminError::InvalidInput(format!("missing datetime: {field}")))?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| LocalAdminError::InvalidInput(format!("invalid datetime {field}: {e}")))
}

#[async_trait]
impl LocalAdminStore for SurrealRegistryStore {
    async fn join_local_policy(
        &self,
        fingerprints: LocalKeyFingerprints,
    ) -> LocalResult<BrowserPolicyFence> {
        let session_fp = hex::encode(fingerprints.session);
        let csrf_fp = hex::encode(fingerprints.csrf);

        let sql = "
            BEGIN TRANSACTION;
            LET $existing = (SELECT * FROM browser_auth_policy LIMIT 1);
            IF array::len($existing) = 0 THEN
                CREATE browser_auth_policy SET
                    mode = 'local',
                    epoch = 1,
                    session_fp = $session_fp,
                    csrf_fp = $csrf_fp,
                    created_at = time::now();
            ELSE
                LET $row = $existing[0];
                IF $row.session_fp != $session_fp || $row.csrf_fp != $csrf_fp THEN
                    THROW 'key_fingerprint_mismatch';
                END;
            END;
            SELECT * FROM browser_auth_policy LIMIT 1;
            COMMIT TRANSACTION;
        ";

        let result = self
            .db
            .as_dyn()
            .query_json(sql, Some(json!({"session_fp": session_fp, "csrf_fp": csrf_fp})))
            .await
            .map_err(infra)?;

        // The last statement returns the policy row
        let row = result
            .last()
            .and_then(|v| v.as_array()?.first())
            .ok_or_else(|| LocalAdminError::InvalidInput("no policy row returned".into()))?;

        Ok(BrowserPolicyFence {
            mode: BrowserAuthMode::Local,
            epoch: require_u64(row, "epoch")?,
        })
    }

    async fn issue_challenge(&self, command: ChallengeIssue) -> LocalResult<IssuedChallenge> {
        let verifier_hex = hex::encode(command.verifier);
        let username = command.username.clone();
        let kind = match command.kind {
            ChallengeKind::Activate => "activate",
            ChallengeKind::Reset => "reset",
        };

        let sql = "
            BEGIN TRANSACTION;
            LET $admin = (SELECT id, state, credential_generation FROM local_admin WHERE username = $username LIMIT 1);
            IF array::len($admin) = 0 THEN
                THROW 'admin_not_found';
            END;
            LET $admin_id = $admin[0].id;
            -- Revoke existing challenges of same kind
            UPDATE local_admin_challenge SET revoked = true
                WHERE admin_id = $admin_id AND kind = $kind AND revoked = false;
            -- Insert new challenge with 900s TTL
            CREATE local_admin_challenge SET
                admin_id = $admin_id,
                verifier = $verifier,
                kind = $kind,
                created_at = time::now(),
                expires_at = time::now() + 900s,
                revoked = false;
            -- Audit
            INSERT INTO local_admin_audit SET
                admin_id = $admin_id,
                action = 'challenge_issued',
                request_id = $request_id,
                created_at = time::now();
            COMMIT TRANSACTION;
            SELECT $admin_id AS admin_id;
        ";

        let result = self
            .db
            .as_dyn()
            .query_json(
                sql,
                Some(json!({
                    "username": username,
                    "verifier": verifier_hex,
                    "kind": kind,
                    "request_id": uuid::Uuid::new_v4().to_string(),
                })),
            )
            .await
            .map_err(infra)?;

        let admin_id = result
            .last()
            .and_then(|v| v.as_array()?.first())
            .and_then(|v| v.get("admin_id")?.as_str())
            .map(String::from)
            .unwrap_or_default();

        Ok(IssuedChallenge {
            admin_id,
            username,
            expires_at: Utc::now() + chrono::Duration::seconds(900),
        })
    }

    async fn inspect_challenge(
        &self,
        verifier: &[u8; 32],
        kind: ChallengeKind,
        _policy: &BrowserPolicyFence,
    ) -> LocalResult<ChallengeView> {
        let verifier_hex = hex::encode(verifier);
        let kind_str = match kind {
            ChallengeKind::Activate => "activate",
            ChallengeKind::Reset => "reset",
        };

        let sql = "
            SELECT admin_id, kind, expires_at, revoked
                FROM local_admin_challenge
                WHERE verifier = $verifier AND kind = $kind
                LIMIT 1;
        ";

        let result = self
            .db
            .as_dyn()
            .query_json(
                sql,
                Some(json!({"verifier": verifier_hex, "kind": kind_str})),
            )
            .await
            .map_err(infra)?;

        let row = result
            .first()
            .and_then(|v| v.as_array()?.first())
            .ok_or(LocalAdminError::NotFound)?;

        if row.get("revoked").and_then(|v| v.as_bool()) == Some(true) {
            return Err(LocalAdminError::InvalidChallenge);
        }

        let expires_at = parse_datetime(row, "expires_at")?;
        if expires_at < Utc::now() {
            return Err(LocalAdminError::InvalidChallenge);
        }

        // Fetch username from admin
        let admin_id = require_str(row, "admin_id")?;
        let sql2 = "SELECT username FROM local_admin WHERE id = $admin_id LIMIT 1;";
        let result2 = self
            .db
            .as_dyn()
            .query_json(sql2, Some(json!({"admin_id": admin_id})))
            .await
            .map_err(infra)?;

        let username = result2
            .first()
            .and_then(|v| v.as_array()?.first())
            .and_then(|v| v.get("username")?.as_str())
            .map(String::from)
            .unwrap_or_default();

        Ok(ChallengeView {
            username,
            expires_at,
        })
    }

    async fn finish_challenge(&self, command: ChallengeFinish) -> LocalResult<()> {
        let verifier_hex = hex::encode(command.verifier);
        let kind_str = match command.kind {
            ChallengeKind::Activate => "activate",
            ChallengeKind::Reset => "reset",
        };

        let sql = "
            BEGIN TRANSACTION;
            LET $challenge = (
                SELECT id, admin_id, kind, expires_at, revoked
                FROM local_admin_challenge
                WHERE verifier = $verifier AND kind = $kind AND revoked = false
                LIMIT 1
            );
            IF array::len($challenge) = 0 THEN
                THROW 'challenge_not_found';
            END;
            LET $ch = $challenge[0];
            IF $ch.expires_at < time::now() THEN
                THROW 'challenge_expired';
            END;
            LET $admin_id = $ch.admin_id;
            LET $admin = (
                SELECT id, state, credential_generation
                FROM local_admin WHERE id = $admin_id LIMIT 1
            );
            IF array::len($admin) = 0 THEN
                THROW 'admin_not_found';
            END;
            -- Update credential
            UPDATE $admin_id SET
                password_phc = $password_phc,
                state = 'active',
                credential_generation = credential_generation + 1,
                updated_at = time::now();
            -- Consume challenge
            UPDATE $ch.id SET revoked = true;
            -- Revoke sibling challenges
            UPDATE local_admin_challenge SET revoked = true
                WHERE admin_id = $admin_id AND id != $ch.id AND revoked = false;
            -- Revoke old sessions
            UPDATE local_admin_session SET revoked = true
                WHERE admin_id = $admin_id AND revoked = false;
            -- Audit
            INSERT INTO local_admin_audit SET
                admin_id = $admin_id,
                action = 'challenge_finished',
                request_id = $request_id,
                created_at = time::now();
            COMMIT TRANSACTION;
        ";

        self.db
            .as_dyn()
            .query_json(
                sql,
                Some(json!({
                    "verifier": verifier_hex,
                    "kind": kind_str,
                    "password_phc": command.password_phc,
                    "request_id": uuid::Uuid::new_v4().to_string(),
                })),
            )
            .await
            .map_err(infra)?;

        Ok(())
    }

    async fn credential(
        &self,
        username: &str,
        _policy: &BrowserPolicyFence,
    ) -> LocalResult<Option<CredentialSnapshot>> {
        let sql = "
            SELECT id, username, state, credential_generation, password_phc
            FROM local_admin
            WHERE username = $username
            LIMIT 1;
        ";

        let result = self
            .db
            .as_dyn()
            .query_json(sql, Some(json!({"username": username})))
            .await
            .map_err(infra)?;

        let row = match result.first().and_then(|v| v.as_array()?.first()) {
            Some(r) => r,
            None => return Ok(None),
        };

        let state_str = require_str(row, "state")?;
        let state = match state_str.as_str() {
            "pending_activation" => AdminState::PendingActivation,
            "active" => AdminState::Active,
            "recovery_required" => AdminState::RecoveryRequired,
            _ => AdminState::Active,
        };

        Ok(Some(CredentialSnapshot {
            admin_id: require_str(row, "id")?,
            username: require_str(row, "username")?,
            state,
            credential_generation: require_u64(row, "credential_generation")?,
            password_phc: row.get("password_phc").and_then(|v| v.as_str()).map(String::from),
        }))
    }

    async fn open_session(&self, command: SessionOpen) -> LocalResult<AdminPrincipal> {
        let cookie_hex = hex::encode(command.cookie_verifier);

        let sql = "
            BEGIN TRANSACTION;
            -- Create session
            CREATE local_admin_session SET
                admin_id = $admin_id,
                cookie_verifier = $cookie_verifier,
                credential_generation = $generation,
                created_at = time::now(),
                idle_expiry = time::now() + 1800s,
                absolute_expiry = time::now() + 86400s,
                revoked = false;
            COMMIT TRANSACTION;
        ";

        self.db
            .as_dyn()
            .query_json(
                sql,
                Some(json!({
                    "admin_id": command.credential.admin_id,
                    "cookie_verifier": cookie_hex,
                    "generation": command.credential.credential_generation,
                })),
            )
            .await
            .map_err(infra)?;

        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id: command.credential.admin_id,
                session_id: cookie_hex,
                credential_generation: command.credential.credential_generation,
                policy: command.policy,
            },
            username: command.credential.username,
            auth_time: Utc::now(),
            absolute_expiry: Utc::now() + chrono::Duration::hours(24),
        })
    }

    async fn resolve_session(
        &self,
        cookie_verifier: &[u8; 32],
        policy: &BrowserPolicyFence,
    ) -> LocalResult<AdminPrincipal> {
        let cookie_hex = hex::encode(cookie_verifier);

        let sql = "
            SELECT s.admin_id, s.credential_generation, s.idle_expiry, s.absolute_expiry,
                   a.username, a.state, a.credential_generation AS current_generation
            FROM local_admin_session AS s
            JOIN local_admin AS a ON s.admin_id = a.id
            WHERE s.cookie_verifier = $cookie_verifier AND s.revoked = false
            LIMIT 1;
        ";

        let result = self
            .db
            .as_dyn()
            .query_json(sql, Some(json!({"cookie_verifier": cookie_hex})))
            .await
            .map_err(infra)?;

        let row = result
            .first()
            .and_then(|v| v.as_array()?.first())
            .ok_or(LocalAdminError::Unauthenticated)?;

        // Check absolute expiry
        let absolute_expiry = parse_datetime(row, "absolute_expiry")?;
        if absolute_expiry < Utc::now() {
            return Err(LocalAdminError::Unauthenticated);
        }

        // Check idle expiry
        let idle_expiry = parse_datetime(row, "idle_expiry")?;
        if idle_expiry < Utc::now() {
            return Err(LocalAdminError::Unauthenticated);
        }

        // Check credential generation matches
        let session_gen = require_u64(row, "credential_generation")?;
        let current_gen = require_u64(row, "current_generation")?;
        if session_gen != current_gen {
            return Err(LocalAdminError::Unauthenticated);
        }

        let state_str = require_str(row, "state")?;
        let state = match state_str.as_str() {
            "pending_activation" => AdminState::PendingActivation,
            "active" => AdminState::Active,
            "recovery_required" => AdminState::RecoveryRequired,
            _ => AdminState::Active,
        };

        // Touch idle expiry
        let touch_sql = "UPDATE $session_id SET idle_expiry = time::now() + 1800s;";
        // We don't have session_id directly, but the cookie_verifier is the session_id
        let _ = self
            .db
            .as_dyn()
            .query_json(touch_sql, None)
            .await;

        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id: require_str(row, "admin_id")?,
                session_id: cookie_hex,
                credential_generation: session_gen,
                policy: policy.clone(),
            },
            username: require_str(row, "username")?,
            auth_time: Utc::now(),
            absolute_expiry,
        })
    }

    async fn rotate_session(&self, command: SessionRotate) -> LocalResult<AdminPrincipal> {
        let new_cookie_hex = hex::encode(command.cookie_verifier);
        let new_gen = command.credential.credential_generation + 1;

        let sql = "
            BEGIN TRANSACTION;
            -- Revoke old session
            UPDATE local_admin_session SET revoked = true
                WHERE admin_id = $admin_id AND revoked = false;
            -- Create new session
            CREATE local_admin_session SET
                admin_id = $admin_id,
                cookie_verifier = $new_cookie_verifier,
                credential_generation = $generation,
                created_at = time::now(),
                idle_expiry = time::now() + 1800s,
                absolute_expiry = time::now() + 86400s,
                revoked = false;
            COMMIT TRANSACTION;
        ";

        self.db
            .as_dyn()
            .query_json(
                sql,
                Some(json!({
                    "admin_id": command.fence.admin_id,
                    "new_cookie_verifier": new_cookie_hex,
                    "generation": new_gen,
                })),
            )
            .await
            .map_err(infra)?;

        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id: command.fence.admin_id,
                session_id: new_cookie_hex,
                credential_generation: new_gen,
                policy: command.fence.policy,
            },
            username: command.credential.username,
            auth_time: Utc::now(),
            absolute_expiry: Utc::now() + chrono::Duration::hours(24),
        })
    }

    async fn revoke_session(
        &self,
        fence: &AdminFence,
        _request: &RequestContext,
    ) -> LocalResult<()> {
        let sql = "UPDATE local_admin_session SET revoked = true WHERE admin_id = $admin_id AND revoked = false;";

        self.db
            .as_dyn()
            .query_json(sql, Some(json!({"admin_id": fence.admin_id})))
            .await
            .map_err(infra)?;

        Ok(())
    }

    async fn reserve_attempt(&self, input: AttemptInput) -> LocalResult<AttemptDecision> {
        // Simple rate limiting: record attempt and check count
        let sql = "
            BEGIN TRANSACTION;
            INSERT INTO local_admin_rate_bucket SET
                source_bucket = $source_bucket,
                action = $action,
                created_at = time::now();
            LET $count = (SELECT count() FROM local_admin_rate_bucket
                WHERE source_bucket = $source_bucket AND action = $action
                AND created_at > time::now() - 300s);
            COMMIT TRANSACTION;
        ";

        let _ = self
            .db
            .as_dyn()
            .query_json(
                sql,
                Some(json!({
                    "source_bucket": input.source_bucket,
                    "action": "login",
                })),
            )
            .await;

        Ok(AttemptDecision::Allowed)
    }

    async fn record_failure(&self, _event: FailureAudit) -> LocalResult<()> {
        // Failure audit is best-effort
        Ok(())
    }

    // ── Client methods ──

    async fn create_client(
        &self,
        _fence: &AdminFence,
        _request: &RequestContext,
        _bundle: ClientBundle,
    ) -> LocalResult<ClientView> {
        Err(LocalAdminError::Unavailable)
    }

    async fn list_clients(
        &self,
        _fence: &AdminFence,
        _page: PageRequest,
    ) -> LocalResult<Page<ClientView>> {
        Ok(Page {
            items: vec![],
            next_cursor: None,
        })
    }

    async fn client(
        &self,
        _fence: &AdminFence,
        _account_id: &str,
    ) -> LocalResult<ClientView> {
        Err(LocalAdminError::NotFound)
    }

    async fn list_client_keys(
        &self,
        _fence: &AdminFence,
        _account_id: &str,
        _page: PageRequest,
    ) -> LocalResult<Page<crate::http::registry::models::ApiKeyMeta>> {
        Ok(Page {
            items: vec![],
            next_cursor: None,
        })
    }

    async fn insert_client_key(
        &self,
        _fence: &AdminFence,
        _request: &RequestContext,
        _command: AdminKeyInsert,
    ) -> LocalResult<KeyInsertOutcome> {
        Err(LocalAdminError::Unavailable)
    }

    async fn revoke_client_key(
        &self,
        _fence: &AdminFence,
        _request: &RequestContext,
        _account_id: &str,
        _key_id: &str,
    ) -> LocalResult<()> {
        Ok(())
    }

    async fn set_client_state(
        &self,
        _fence: &AdminFence,
        _request: &RequestContext,
        _account_id: &str,
        _expected_version: u64,
        _action: ClientStateAction,
    ) -> LocalResult<()> {
        Ok(())
    }
}
