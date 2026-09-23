//! Durable `LocalAdminStore` implementation for `SurrealRegistryStore`.
//!
//! Each method owns a constant SQL body, a documented constant result
//! index, and a strict decoder. All user-controlled values are bound
//! parameters; the only interpolated identifiers are fixed schema names.
//!
//! SQL notes for SurrealDB 3.2.4 (verified empirically against the
//! migration in `migrations/047_local_admin_auth.surql`):
//!
//! * Compound conditionals must use brace blocks; `IF … { … } ELSE IF …
//!   { … } ELSE { … }` and nested blocks parse, the unbraced `… END` form
//!   does not.
//! * A `THROW` inside a transaction aborts it; every statement error is
//!   surfaced by the adapter before any result is decoded.
//! * `BEGIN`/`LET`/`IF`/`COMMIT` each occupy one result index (with no
//!   value); `CREATE`/`UPDATE`/`SELECT`/`RETURN` produce the value read
//!   by [`query_json_at`]. Indices below are counted over the constant
//!   statement sequence, including the guard fragment.
//! * `record::id($record)` yields the bare key, which is what the string
//!   `admin_id` columns store; records are addressed with
//!   `type::record('table', $key)`.
//!
//! [`query_json_at`]: crate::http::registry::surreal_store::SurrealHandle::query_json_at

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Map, Value, json};

use crate::error::MemoryError;
use crate::http::registry::models::{AccountStatus, ApiKeyMeta, TenantStatus};
use crate::service::local_admin::contracts::{
    AdminFence, AdminKeyInsert, AdminPrincipal, AdminState, AttemptDecision, AttemptDomain,
    AttemptInput, BrowserPolicyFence, ChallengeFinish, ChallengeIssue, ChallengeKind,
    ChallengeView, ClientBundle, ClientStateAction, ClientView, CredentialSnapshot, FailureAction,
    FailureAudit, FailureReason, IssuedChallenge, KeyExpiry, KeyInsertOutcome, LocalAdminError,
    LocalAdminStore, LocalResult, Page, PageRequest, RequestContext, SessionOpen, SessionRotate,
};

use super::{SurrealRegistryStore, record_id_value, row_id as decode_record_id, status_from_row};

/// Credential attempts allowed per username bucket per fixed 15-minute window.
const CREDENTIAL_USERNAME_CAP: u64 = 5;
const CREDENTIAL_USERNAME_WINDOW: &str = "900s";
/// Credential attempts allowed per source bucket per fixed 5-minute window.
const CREDENTIAL_SOURCE_CAP: u64 = 30;
const CREDENTIAL_SOURCE_WINDOW: &str = "300s";
/// Challenge attempts allowed per source bucket per fixed 15-minute window.
const CHALLENGE_SOURCE_CAP: u64 = 10;
const CHALLENGE_SOURCE_WINDOW: &str = "900s";
/// Highest valid throttle bucket (inclusive); storage cardinality is bounded
/// to 4096 slots per dimension.
const MAX_BUCKET: u16 = 4095;

/// Whether a storage error is a lost write-write race that SurrealDB aborted
/// atomically, and which may therefore be retried.
///
/// SurrealDB aborts the whole transaction on a commit conflict, so a retry
/// re-reads the persisted state and re-applies its single increment; it cannot
/// partially commit or double count.
fn is_write_conflict(error: &MemoryError) -> bool {
    matches!(error, MemoryError::Storage(message)
        if message.contains("Write conflict") || message.contains("can be retried"))
}

/// Retry budget for a transaction that loses a write-write race.
const CONFLICT_RETRIES: u32 = 5;

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
        .or_else(|| {
            value
                .get(field)
                .and_then(|v| v.get("Datetime").and_then(Value::as_str))
        })
        .ok_or_else(|| LocalAdminError::InvalidInput(format!("missing datetime: {field}")))?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| LocalAdminError::InvalidInput(format!("invalid datetime {field}: {e}")))
}

/// Extract the first allowlisted `THROW` sentinel from a storage error.
///
/// Callers map the returned token to a typed domain error and fall back to
/// [`infra`] otherwise, so raw SQL text never reaches a caller.
fn thrown<'a>(error: &MemoryError, sentinels: &[&'a str]) -> Option<&'a str> {
    if let MemoryError::Storage(message) = error {
        for token in sentinels {
            if message.contains(*token) {
                return Some(token);
            }
        }
    }
    None
}

/// Encode a serialisable value for a bound parameter.
fn encode<T: serde::Serialize>(value: &T, label: &str) -> LocalResult<Value> {
    serde_json::to_value(value)
        .map_err(|error| infra(MemoryError::Storage(format!("encode {label}: {error}"))))
}

/// Strictly decode a stored status string.
fn decode_status<T: serde::de::DeserializeOwned>(row: &Value, field: &str) -> LocalResult<T> {
    status_from_row(row, field).map_err(infra)
}

/// Decode the durable administrator state.
fn decode_admin_state(raw: &str) -> LocalResult<AdminState> {
    match raw {
        "pending_activation" => Ok(AdminState::PendingActivation),
        "active" => Ok(AdminState::Active),
        "recovery_required" => Ok(AdminState::RecoveryRequired),
        other => Err(infra(MemoryError::Storage(format!(
            "local_admin row has invalid state: {other}"
        )))),
    }
}

/// The live `account`/`tenant` overlay appended to every client projection.
///
/// `local_admin_client` is a workflow sidecar: it owns the CAS guard
/// `version` and the local-admin ownership scope, but it is only a
/// point-in-time snapshot of provisioning state. The `account` and `tenant`
/// rows written by the provisioning worker are authoritative, so the
/// projection fetches them alongside the sidecar.
///
/// `$parent` is scoped to the *immediately enclosing* statement, so the
/// tenant lookup cannot name the client row directly: it has to walk
/// `local_admin_client → account → tenant`, invoking `$parent` once per
/// level. Writing `$parent.account_id` inside the tenant subquery silently
/// yields `NONE` (the account row has no `account_id` field) and the overlay
/// disappears rather than failing.
const CLIENT_LIVE_PROJECTION: &str = r#"
           (SELECT VALUE tenant_id FROM account WHERE id = type::record('account', $parent.account_id) LIMIT 1)[0] AS tenant_id,
           (SELECT VALUE status FROM account WHERE id = type::record('account', $parent.account_id) LIMIT 1)[0] AS live_account_status,
           (SELECT VALUE (SELECT status, plan_version, schema_version, retry_stage FROM tenant
                          WHERE id = type::record('tenant', $parent.tenant_id) LIMIT 1)[0]
            FROM account WHERE id = type::record('account', $parent.account_id) LIMIT 1)[0] AS live_tenant
"#;

/// The live tenant fields the view needs.
#[derive(serde::Deserialize)]
struct LiveTenant {
    status: TenantStatus,
    plan_version: u32,
    schema_version: u32,
    #[serde(default)]
    retry_stage: Option<TenantStatus>,
}

/// Decode a bare status token.
fn decode_status_token<T: serde::de::DeserializeOwned>(raw: &str, label: &str) -> LocalResult<T> {
    serde_json::from_value(Value::String(raw.to_owned()))
        .map_err(|error| infra(MemoryError::Storage(format!("decode {label}: {error}"))))
}

/// Closed mapping from live provisioning state to a safe, bounded reason.
///
/// Only allowlisted stage tokens are rendered, so a raw storage or migration
/// error can never reach a caller through this field.
fn provisioning_reason(status: TenantStatus, retry_stage: Option<TenantStatus>) -> Option<String> {
    match status {
        TenantStatus::Failed => Some(match retry_stage {
            Some(stage) => format!("provisioning failed at {}", stage_token(stage)),
            None => "provisioning failed".to_owned(),
        }),
        TenantStatus::Reserved | TenantStatus::NamespaceCreating | TenantStatus::Migrating => {
            retry_stage.map(|stage| format!("retrying from {}", stage_token(stage)))
        }
        TenantStatus::Ready
        | TenantStatus::Suspended
        | TenantStatus::Deleting
        | TenantStatus::Purged => None,
    }
}

/// Allowlisted token for one provisioning stage.
fn stage_token(status: TenantStatus) -> &'static str {
    match status {
        TenantStatus::Reserved => "reserved",
        TenantStatus::NamespaceCreating => "namespace_creating",
        TenantStatus::Migrating => "migrating",
        TenantStatus::Ready => "ready",
        TenantStatus::Suspended => "suspended",
        TenantStatus::Failed => "failed",
        TenantStatus::Deleting => "deleting",
        TenantStatus::Purged => "purged",
    }
}

/// Decode a `local_admin_client` row plus its live `account`/`tenant` overlay.
///
/// Live values win when the referenced row is readable; the sidecar snapshot
/// is the fallback, because a client whose account vanished must still render
/// rather than fail the whole page. The CAS guard `version` always comes from
/// the sidecar row — the tenant carries its own independent version.
fn decode_client_view(row: &Value) -> LocalResult<ClientView> {
    let live_tenant: Option<LiveTenant> = match row.get("live_tenant").filter(|v| v.is_object()) {
        Some(value) => Some(serde_json::from_value(value.clone()).map_err(|error| {
            infra(MemoryError::Storage(format!("decode live tenant: {error}")))
        })?),
        None => None,
    };

    let account_status = match row.get("live_account_status").and_then(Value::as_str) {
        Some(raw) => decode_status_token::<AccountStatus>(raw, "live account status")?,
        None => decode_status::<AccountStatus>(row, "account_status")?,
    };

    let tenant_status = match live_tenant.as_ref() {
        Some(tenant) => tenant.status,
        None => decode_status::<TenantStatus>(row, "tenant_status")?,
    };

    // A live record that predates a field falls back to the sidecar snapshot.
    let plan_version = match live_tenant.as_ref() {
        Some(tenant) => tenant.plan_version,
        None => u32::try_from(require_u64(row, "plan_version")?).map_err(|_| {
            infra(MemoryError::Storage(
                "client plan_version out of range".into(),
            ))
        })?,
    };
    let schema_version = match live_tenant.as_ref() {
        Some(tenant) => tenant.schema_version,
        None => u32::try_from(require_u64(row, "schema_version")?).map_err(|_| {
            infra(MemoryError::Storage(
                "client schema_version out of range".into(),
            ))
        })?,
    };

    let provisioning_reason = live_tenant
        .as_ref()
        .and_then(|tenant| provisioning_reason(tenant.status, tenant.retry_stage));

    Ok(ClientView {
        account_id: require_str(row, "account_id")?,
        tenant_id: require_str(row, "tenant_id")?,
        display_name: require_str(row, "display_name")?,
        account_status,
        tenant_status,
        plan_version,
        schema_version,
        version: require_u64(row, "version")?,
        provisioning_reason,
    })
}

/// Serialise a failure action to its allowlisted audit token.
fn failure_action_token(action: FailureAction) -> &'static str {
    match action {
        FailureAction::Login => "login",
        FailureAction::Reauth => "reauth",
        FailureAction::Challenge => "challenge",
        FailureAction::Session => "session",
        FailureAction::ClientMutation => "client_mutation",
    }
}

/// Serialise a failure reason to its allowlisted audit token.
fn failure_reason_token(reason: FailureReason) -> &'static str {
    match reason {
        FailureReason::InvalidCredentials => "invalid_credentials",
        FailureReason::InvalidChallenge => "invalid_challenge",
        FailureReason::InvalidSession => "invalid_session",
        FailureReason::StaleFence => "stale_fence",
        FailureReason::Forbidden => "forbidden",
    }
}

/// Shared mutation guard: policy → admin → presented session, then the
/// conflict writes the spec requires.
///
/// It is a constant 8-statement fragment, so within any guarded mutation
/// transaction it occupies result indices 1..=8 (index 0 is the opening
/// `BEGIN TRANSACTION`) and the first method-specific statement is index 9.
/// The two `UPDATE`s touch the admin `version` and the session row so a
/// concurrent recovery, logout or rotation aborts the transaction. The
/// session touch matches every live session, including the final 30 minutes
/// of absolute lifetime, and caps the idle deadline at the absolute one.
const MUTATION_GUARD: &str = r#"
LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };
LET $guard_admin = (SELECT state, credential_generation FROM local_admin WHERE id = type::record('local_admin', $admin_id) LIMIT 1);
IF array::len($guard_admin) = 0 OR $guard_admin[0].state != 'active' OR $guard_admin[0].credential_generation != $generation { THROW 'admin_stale'; };
LET $guard_session = (SELECT id FROM local_admin_session WHERE cookie_verifier = $session_verifier AND admin_id = $admin_id AND credential_generation = $generation AND mode_epoch = $epoch AND revoked_at IS NONE AND idle_expiry > time::now() AND absolute_expiry > time::now() AND auth_time > time::now() - 600s LIMIT 1);
IF array::len($guard_session) = 0 { THROW 'session_invalid'; };
UPDATE type::record('local_admin', $admin_id) SET version = version + 1, updated_at = time::now();
UPDATE local_admin_session SET idle_expiry = IF absolute_expiry > time::now() + 1800s { time::now() + 1800s } ELSE { absolute_expiry } WHERE cookie_verifier = $session_verifier AND revoked_at IS NONE;
"#;

/// Bind the guard parameters shared by every guarded mutation.
fn guarded_vars(fence: &AdminFence) -> Map<String, Value> {
    let mut vars = Map::new();
    vars.insert("epoch".into(), json!(fence.policy.epoch));
    vars.insert("admin_id".into(), json!(fence.admin_id.clone()));
    vars.insert("generation".into(), json!(fence.credential_generation));
    vars.insert("session_verifier".into(), json!(fence.session_id.clone()));
    vars
}

/// Guard variables plus method-specific parameters.
fn with_guard(fence: &AdminFence, extra: Value) -> Value {
    let mut vars = guarded_vars(fence);
    if let Value::Object(extra) = extra {
        vars.extend(extra);
    }
    Value::Object(vars)
}

/// A fresh opaque id for a durable row.
fn row_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// Stable bucket id for one throttle dimension slot.
fn bucket_id(domain: &str, dimension: &str, bucket: u16) -> String {
    format!("{domain}:{dimension}:{bucket}")
}

impl SurrealRegistryStore {
    /// The single execution seam for every local-admin statement.
    ///
    /// Routing the whole local-admin surface through two helpers keeps the
    /// SQL fault hook (plan §5) in one place and makes it verifiable that no
    /// local-admin statement reaches the engine while bypassing it.
    ///
    /// Both helpers also absorb a lost write race. SurrealDB aborts a
    /// transaction that loses a write-write race (two administrators acting on
    /// one client, two admissions touching one bucket) instead of blocking on
    /// it, so without a retry a benign concurrent admin action would surface as
    /// a spurious `503 temporarily_unavailable`. Retrying is safe because each
    /// statement here is a single atomic transaction — the guard-row update,
    /// the resource write, the idempotency operation and the audit row either
    /// all commit or none do — so an aborted attempt wrote nothing and the
    /// re-run re-reads the persisted state. A read cannot lose a write race, so
    /// the retry never fires for one.
    pub(super) async fn admin_query(
        &self,
        sql: &str,
        vars: Option<Value>,
    ) -> Result<Vec<Value>, MemoryError> {
        self.admin_query_with_retry(sql, vars, None).await
    }

    /// [`Self::admin_query`] with the explicit result index.
    pub(super) async fn admin_query_at(
        &self,
        sql: &str,
        vars: Option<Value>,
        result_index: usize,
    ) -> Result<Vec<Value>, MemoryError> {
        self.admin_query_with_retry(sql, vars, Some(result_index))
            .await
    }

    async fn admin_query_with_retry(
        &self,
        sql: &str,
        vars: Option<Value>,
        result_index: Option<usize>,
    ) -> Result<Vec<Value>, MemoryError> {
        let mut conflicts = 0_u32;
        loop {
            self.sql_faults.check(sql)?;
            let attempt = match result_index {
                Some(index) => {
                    self.db
                        .as_dyn()
                        .query_json_at(sql, vars.clone(), index)
                        .await
                }
                None => self.db.as_dyn().query_json(sql, vars.clone()).await,
            };
            match attempt {
                Ok(rows) => return Ok(rows),
                Err(error) if conflicts < CONFLICT_RETRIES && is_write_conflict(&error) => {
                    conflicts += 1;
                    // 2, 4, 8, 16, 32 ms: enough for the winning transaction to
                    // finish, short enough not to hold a request open.
                    tokio::time::sleep(std::time::Duration::from_millis(1u64 << conflicts.min(5)))
                        .await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Arm the SQL fault hook so the next local-admin statement whose SQL
    /// contains `needle` fails before execution. Test-only: nothing in the
    /// production composition calls it.
    #[cfg(test)]
    pub(crate) fn arm_sql_fault(&self, needle: &str) {
        self.sql_faults.arm(needle);
    }
}

#[async_trait]
impl LocalAdminStore for SurrealRegistryStore {
    /// Statement order:
    /// `BEGIN`(0) `LET`(1, policy) `IF`(2, policy) `LET`(3) `IF`(4)
    /// `IF`(5) `IF`(6) `LET`(7) `LET`(8) `IF`(9) `IF`(10) `IF`(11)
    /// `UPDATE`(12) `LET`(13) `CREATE`(14) `CREATE`(15) `SELECT`(16)
    /// `COMMIT`(17).
    /// Result index 16 is the created challenge readback.
    async fn issue_challenge(&self, command: ChallengeIssue) -> LocalResult<IssuedChallenge> {
        let kind = match command.kind {
            ChallengeKind::Activate => "activate",
            ChallengeKind::Reset => "reset",
        };
        let username = command.username;
        let admin_id = row_id("adm");
        let challenge_id = row_id("chg");
        let audit_id = row_id("aud");

        let sql = "
            BEGIN TRANSACTION;
            LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };
            LET $existing = (SELECT id, state, credential_generation FROM local_admin WHERE username = $username LIMIT 1);
            IF array::len($existing) = 0 AND $kind = 'reset' { THROW 'admin_not_found'; };
            IF array::len($existing) = 0 AND $kind = 'activate' {
                CREATE type::record('local_admin', $admin_id) SET
                    id = $admin_id,
                    username = $username,
                    state = 'pending_activation',
                    password_phc = NONE,
                    credential_generation = 1,
                    version = 1,
                    created_at = time::now(),
                    updated_at = time::now();
            };
            IF array::len($existing) > 0 AND $kind = 'activate' {
                THROW 'admin_already_exists';
            };
            LET $admin = (SELECT id, state, credential_generation FROM local_admin WHERE username = $username LIMIT 1);
            LET $admin_key = <string> record::id($admin[0].id);
            IF $kind = 'reset' {
                UPDATE type::record('local_admin', $admin_key) SET
                    state = 'recovery_required',
                    credential_generation = credential_generation + 1,
                    version = version + 1,
                    updated_at = time::now();
            };
            IF $kind = 'reset' {
                UPDATE local_admin_session SET revoked_at = time::now()
                    WHERE admin_id = $admin_key AND revoked_at IS NONE;
            };
            IF $kind = 'reset' {
                UPDATE local_admin_challenge SET revoked_at = time::now()
                    WHERE admin_id = $admin_key AND revoked_at IS NONE;
            };
            UPDATE local_admin_challenge SET revoked_at = time::now()
                WHERE admin_id = $admin_key AND kind = $kind AND revoked_at IS NONE;
            LET $generation = (SELECT credential_generation FROM local_admin WHERE id = type::record('local_admin', $admin_key) LIMIT 1);
            CREATE type::record('local_admin_challenge', $challenge_id) SET
                id = $challenge_id,
                admin_id = $admin_key,
                kind = $kind,
                verifier = $verifier,
                credential_generation = $generation[0].credential_generation,
                mode_epoch = $epoch,
                expires_at = time::now() + 900s,
                consumed_at = NONE,
                revoked_at = NONE,
                created_at = time::now();
            CREATE type::record('local_admin_audit', $audit_id) SET
                id = $audit_id,
                event_time = time::now(),
                actor_kind = 'cli',
                actor_id = 'local_admin_cli',
                action = 'challenge_issued',
                target_admin_id = $admin_key,
                target_account_id = NONE,
                target_tenant_id = NONE,
                target_key_id = NONE,
                outcome = 'success',
                reason = NONE,
                request_id = $request_id,
                grants_client_data_access = NONE;
            SELECT admin_id, expires_at FROM local_admin_challenge WHERE id = type::record('local_admin_challenge', $challenge_id) LIMIT 1;
            COMMIT TRANSACTION;
        ";

        let rows = self
            .admin_query_at(
                sql,
                Some(json!({
                    "username": username.clone(),
                    "kind": kind,
                    "verifier": hex::encode(command.verifier),
                    "epoch": command.policy.epoch,
                    "admin_id": admin_id,
                    "challenge_id": challenge_id,
                    "audit_id": audit_id,
                    "request_id": command.request.request_id.to_string(),
                })),
                16,
            )
            .await
            .map_err(|error| {
                match thrown(
                    &error,
                    &["admin_not_found", "admin_already_exists", "policy_stale"],
                ) {
                    Some("admin_not_found") => LocalAdminError::NotFound,
                    Some(_) => LocalAdminError::StateConflict,
                    None => infra(error),
                }
            })?;

        let row = rows.into_iter().next().ok_or_else(|| {
            infra(MemoryError::Storage(
                "issue_challenge returned no challenge row".into(),
            ))
        })?;

        Ok(IssuedChallenge {
            admin_id: require_str(&row, "admin_id")?,
            username,
            expires_at: parse_datetime(&row, "expires_at")?,
        })
    }

    /// Non-consuming read. Result index 0 is the single challenge row; the
    /// username is a second, separate read. Both use `query_json` because a
    /// zero-row result is a valid `InvalidChallenge` outcome, not a missing
    /// result.
    async fn inspect_challenge(
        &self,
        verifier: &[u8; 32],
        kind: ChallengeKind,
        policy: &BrowserPolicyFence,
    ) -> LocalResult<ChallengeView> {
        let kind_str = match kind {
            ChallengeKind::Activate => "activate",
            ChallengeKind::Reset => "reset",
        };

        // Re-read the durable policy fence first (spec §7: every local auth
        // transaction checks the policy): a fence that has moved rejects the
        // inspection with the same uniform answer as any other bad code.
        self.admin_query(
            "LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
             IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };",
            Some(json!({"epoch": policy.epoch})),
        )
        .await
        .map_err(|error| match thrown(&error, &["policy_stale"]) {
            Some(_) => LocalAdminError::InvalidChallenge,
            None => infra(error),
        })?;

        let sql = "
            SELECT admin_id, expires_at FROM local_admin_challenge
            WHERE verifier = $verifier AND kind = $kind
              AND consumed_at IS NONE AND revoked_at IS NONE
              AND expires_at > time::now() AND mode_epoch = $epoch
            LIMIT 1;
        ";

        let rows = self
            .admin_query(
                sql,
                Some(json!({
                    "verifier": hex::encode(verifier),
                    "kind": kind_str,
                    "epoch": policy.epoch,
                })),
            )
            .await
            .map_err(infra)?;

        let row = rows
            .into_iter()
            .next()
            .ok_or(LocalAdminError::InvalidChallenge)?;
        let admin_id = require_str(&row, "admin_id")?;
        let expires_at = parse_datetime(&row, "expires_at")?;

        let username_rows = self.admin_query(
                "SELECT VALUE username FROM local_admin WHERE id = type::record('local_admin', $admin_id) LIMIT 1;",
                Some(json!({"admin_id": admin_id})),
            )
            .await
            .map_err(infra)?;
        let username = username_rows
            .into_iter()
            .next()
            .and_then(|value| value.as_str().map(String::from))
            .ok_or(LocalAdminError::InvalidChallenge)?;

        Ok(ChallengeView {
            username,
            expires_at,
        })
    }

    /// No result is consumed; every statement error is still validated.
    /// Two concurrent submissions must yield exactly one success because
    /// the challenge is consumed by record id inside the transaction.
    async fn finish_challenge(&self, command: ChallengeFinish) -> LocalResult<()> {
        let kind_str = match command.kind {
            ChallengeKind::Activate => "activate",
            ChallengeKind::Reset => "reset",
        };
        let audit_id = row_id("aud");

        let sql = "
            BEGIN TRANSACTION;
            LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };
            LET $challenge = (SELECT id, admin_id, kind, credential_generation FROM local_admin_challenge WHERE verifier = $verifier AND kind = $kind AND consumed_at IS NONE AND revoked_at IS NONE AND expires_at > time::now() AND mode_epoch = $epoch LIMIT 1);
            IF array::len($challenge) = 0 { THROW 'challenge_invalid'; };
            LET $ch = $challenge[0];
            LET $admin = (SELECT id, state, credential_generation FROM local_admin WHERE id = type::record('local_admin', $ch.admin_id) LIMIT 1);
            IF array::len($admin) = 0 { THROW 'challenge_invalid'; };
            IF $admin[0].credential_generation != $ch.credential_generation { THROW 'challenge_invalid'; };
            IF $kind = 'activate' AND $admin[0].state != 'pending_activation' { THROW 'challenge_invalid'; };
            IF $kind = 'reset' AND $admin[0].state != 'recovery_required' { THROW 'challenge_invalid'; };
            UPDATE type::record('local_admin', $ch.admin_id) SET
                password_phc = $password_phc,
                state = 'active',
                credential_generation = credential_generation + 1,
                version = version + 1,
                updated_at = time::now();
            UPDATE $challenge[0].id SET consumed_at = time::now();
            UPDATE local_admin_challenge SET revoked_at = time::now()
                WHERE admin_id = $ch.admin_id AND revoked_at IS NONE;
            UPDATE local_admin_session SET revoked_at = time::now()
                WHERE admin_id = $ch.admin_id AND revoked_at IS NONE;
            CREATE type::record('local_admin_audit', $audit_id) SET
                id = $audit_id,
                event_time = time::now(),
                actor_kind = 'browser',
                actor_id = $ch.admin_id,
                action = 'challenge_finished',
                target_admin_id = $ch.admin_id,
                target_account_id = NONE,
                target_tenant_id = NONE,
                target_key_id = NONE,
                outcome = 'success',
                reason = NONE,
                request_id = $request_id,
                grants_client_data_access = NONE;
            COMMIT TRANSACTION;
        ";

        self.admin_query(
            sql,
            Some(json!({
                "verifier": hex::encode(command.verifier),
                "kind": kind_str,
                "epoch": command.policy.epoch,
                "password_phc": command.password_phc,
                "audit_id": audit_id,
                "request_id": command.request.request_id.to_string(),
            })),
        )
        .await
        .map_err(
            |error| match thrown(&error, &["challenge_invalid", "policy_stale"]) {
                Some(_) => LocalAdminError::InvalidChallenge,
                None => infra(error),
            },
        )?;

        Ok(())
    }

    /// Result index 0 is the single credential row; an empty result is
    /// `None`, never an error, so the service can take the dummy-KDF path.
    /// `query_json` is used deliberately: it maps a zero-row result to an
    /// empty vector, while `query_json_at` treats it as a missing result.
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

        let rows = self
            .admin_query(sql, Some(json!({"username": username})))
            .await
            .map_err(infra)?;

        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };

        let admin_id = row
            .get("id")
            .and_then(record_id_value)
            .ok_or_else(|| infra(MemoryError::Storage("local_admin row has no id".into())))?;

        Ok(Some(CredentialSnapshot {
            admin_id,
            username: require_str(&row, "username")?,
            state: decode_admin_state(&require_str(&row, "state")?)?,
            credential_generation: require_u64(&row, "credential_generation")?,
            password_phc: row
                .get("password_phc")
                .and_then(|v| v.as_str())
                .map(String::from),
        }))
    }

    /// Statement order: `BEGIN`(0) `LET`(1, policy) `IF`(2, policy)
    /// `LET`(3) `IF`(4) `CREATE`(5) `CREATE`(6) `SELECT`(7) `COMMIT`(8).
    /// Result index 7 is the inserted session readback, so the returned
    /// deadlines are database time.
    async fn open_session(&self, command: SessionOpen) -> LocalResult<AdminPrincipal> {
        let cookie_verifier = hex::encode(command.cookie_verifier);
        let audit_id = row_id("aud");

        let sql = "
            BEGIN TRANSACTION;
            LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };
            LET $admin = (SELECT state, credential_generation, password_phc FROM local_admin WHERE id = type::record('local_admin', $admin_id) LIMIT 1);
            IF array::len($admin) = 0 OR $admin[0].state != 'active' OR $admin[0].credential_generation != $generation OR $admin[0].password_phc != $password_phc {
                THROW 'login_conflict';
            };
            CREATE type::record('local_admin_session', $session_id) SET
                id = $session_id,
                cookie_verifier = $cookie_verifier,
                admin_id = $admin_id,
                credential_generation = $generation,
                mode_epoch = $epoch,
                auth_time = time::now(),
                idle_expiry = time::now() + 1800s,
                absolute_expiry = time::now() + 86400s,
                revoked_at = NONE;
            CREATE type::record('local_admin_audit', $audit_id) SET
                id = $audit_id,
                event_time = time::now(),
                actor_kind = 'admin',
                actor_id = $admin_id,
                action = 'login',
                target_admin_id = $admin_id,
                target_account_id = NONE,
                target_tenant_id = NONE,
                target_key_id = NONE,
                outcome = 'success',
                reason = NONE,
                request_id = $request_id,
                grants_client_data_access = NONE;
            SELECT auth_time, absolute_expiry FROM local_admin_session WHERE id = type::record('local_admin_session', $session_id) LIMIT 1;
            COMMIT TRANSACTION;
        ";

        let rows = self
            .admin_query_at(
                sql,
                Some(json!({
                    "admin_id": command.credential.admin_id.clone(),
                    "generation": command.credential.credential_generation,
                    "password_phc": command.credential.password_phc,
                    "session_id": cookie_verifier.clone(),
                    "cookie_verifier": cookie_verifier.clone(),
                    "epoch": command.policy.epoch,
                    "audit_id": audit_id,
                    "request_id": command.request.request_id.to_string(),
                })),
                7,
            )
            .await
            .map_err(
                |error| match thrown(&error, &["login_conflict", "policy_stale"]) {
                    Some("policy_stale") => LocalAdminError::Unauthenticated,
                    Some(_) => LocalAdminError::InvalidCredentials,
                    None => infra(error),
                },
            )?;

        let row = rows.into_iter().next().ok_or_else(|| {
            infra(MemoryError::Storage(
                "open_session returned no session".into(),
            ))
        })?;

        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id: command.credential.admin_id,
                session_id: cookie_verifier,
                credential_generation: command.credential.credential_generation,
                policy: command.policy,
            },
            username: command.credential.username,
            auth_time: parse_datetime(&row, "auth_time")?,
            absolute_expiry: parse_datetime(&row, "absolute_expiry")?,
        })
    }

    /// Statement order: `BEGIN`(0) `LET`(1, policy) `IF`(2, policy)
    /// `LET`(3) `IF`(4) `LET`(5) `IF`(6) `IF`(7) `LET`(8) `IF`(9)
    /// `UPDATE`(10) `UPDATE`(11) `RETURN`(12) `COMMIT`(13). Result index 12
    /// is the principal projection.
    async fn resolve_session(
        &self,
        cookie_verifier: &[u8; 32],
        policy: &BrowserPolicyFence,
    ) -> LocalResult<AdminPrincipal> {
        let cookie_verifier = hex::encode(cookie_verifier);

        let sql = "
            BEGIN TRANSACTION;
            LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };
            LET $session_row = (SELECT admin_id, credential_generation, mode_epoch, auth_time, idle_expiry, absolute_expiry FROM local_admin_session WHERE cookie_verifier = $cookie_verifier AND revoked_at IS NONE LIMIT 1);
            IF array::len($session_row) = 0 { THROW 'session_invalid'; };
            LET $s = $session_row[0];
            IF $s.mode_epoch != $epoch { THROW 'session_invalid'; };
            IF $s.idle_expiry <= time::now() OR $s.absolute_expiry <= time::now() { THROW 'session_invalid'; };
            LET $admin = (SELECT username, state, credential_generation FROM local_admin WHERE id = type::record('local_admin', $s.admin_id) LIMIT 1);
            IF array::len($admin) = 0 OR $admin[0].state != 'active' OR $admin[0].credential_generation != $s.credential_generation { THROW 'session_invalid'; };
            UPDATE local_admin_session SET idle_expiry = time::now() + 1800s WHERE cookie_verifier = $cookie_verifier AND revoked_at IS NONE AND absolute_expiry > time::now() + 1800s;
            UPDATE local_admin_session SET idle_expiry = $s.absolute_expiry WHERE cookie_verifier = $cookie_verifier AND revoked_at IS NONE AND absolute_expiry <= time::now() + 1800s;
            RETURN { admin_id: $s.admin_id, username: $admin[0].username, credential_generation: $s.credential_generation, auth_time: $s.auth_time, absolute_expiry: $s.absolute_expiry };
            COMMIT TRANSACTION;
        ";

        let rows = self
            .admin_query_at(
                sql,
                Some(json!({"cookie_verifier": cookie_verifier.clone(), "epoch": policy.epoch})),
                12,
            )
            .await
            .map_err(
                |error| match thrown(&error, &["session_invalid", "policy_stale"]) {
                    Some(_) => LocalAdminError::Unauthenticated,
                    None => infra(error),
                },
            )?;

        let row = rows.into_iter().next().ok_or_else(|| {
            infra(MemoryError::Storage(
                "resolve_session returned no principal".into(),
            ))
        })?;
        let admin_id = require_str(&row, "admin_id")?;
        let credential_generation = require_u64(&row, "credential_generation")?;

        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id,
                session_id: cookie_verifier,
                credential_generation,
                policy: policy.clone(),
            },
            username: require_str(&row, "username")?,
            auth_time: parse_datetime(&row, "auth_time")?,
            absolute_expiry: parse_datetime(&row, "absolute_expiry")?,
        })
    }

    /// Statement order: `BEGIN`(0) `LET`(1, policy) `IF`(2, policy)
    /// `LET`(3) `IF`(4) `LET`(5) `IF`(6) `IF`(7) `LET`(8) `IF`(9)
    /// `UPDATE`(10) `CREATE`(11) `CREATE`(12) `RETURN`(13)
    /// `COMMIT`(14). Result index 13 is the principal projection carrying
    /// the preserved absolute deadline.
    async fn rotate_session(&self, command: SessionRotate) -> LocalResult<AdminPrincipal> {
        let old_cookie = command.fence.session_id.clone();
        let new_cookie = hex::encode(command.cookie_verifier);
        let audit_id = row_id("aud");

        let sql = "
            BEGIN TRANSACTION;
            LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };
            LET $old = (SELECT admin_id, credential_generation, mode_epoch, idle_expiry, absolute_expiry FROM local_admin_session WHERE cookie_verifier = $old_cookie_verifier AND revoked_at IS NONE LIMIT 1);
            IF array::len($old) = 0 { THROW 'session_invalid'; };
            LET $o = $old[0];
            IF $o.admin_id != $admin_id OR $o.credential_generation != $fence_generation OR $o.mode_epoch != $epoch { THROW 'session_invalid'; };
            IF $o.idle_expiry <= time::now() OR $o.absolute_expiry <= time::now() { THROW 'session_invalid'; };
            LET $admin = (SELECT username, state FROM local_admin WHERE id = type::record('local_admin', $admin_id) LIMIT 1);
            IF array::len($admin) = 0 OR $admin[0].state != 'active' { THROW 'session_invalid'; };
            UPDATE local_admin_session SET revoked_at = time::now() WHERE cookie_verifier = $old_cookie_verifier AND revoked_at IS NONE;
            CREATE type::record('local_admin_session', $session_id) SET
                id = $session_id,
                cookie_verifier = $new_cookie_verifier,
                admin_id = $admin_id,
                credential_generation = $credential_generation,
                mode_epoch = $epoch,
                auth_time = time::now(),
                idle_expiry = time::now() + 1800s,
                absolute_expiry = $o.absolute_expiry,
                revoked_at = NONE;
            CREATE type::record('local_admin_audit', $audit_id) SET
                id = $audit_id,
                event_time = time::now(),
                actor_kind = 'admin',
                actor_id = $admin_id,
                action = 'reauth',
                target_admin_id = $admin_id,
                target_account_id = NONE,
                target_tenant_id = NONE,
                target_key_id = NONE,
                outcome = 'success',
                reason = NONE,
                request_id = $request_id,
                grants_client_data_access = NONE;
            RETURN { admin_id: $admin_id, username: $admin[0].username, credential_generation: $credential_generation, auth_time: time::now(), absolute_expiry: $o.absolute_expiry };
            COMMIT TRANSACTION;
        ";

        let rows = self
            .admin_query_at(
                sql,
                Some(json!({
                    "admin_id": command.fence.admin_id,
                    "fence_generation": command.fence.credential_generation,
                    "old_cookie_verifier": old_cookie,
                    "new_cookie_verifier": new_cookie.clone(),
                    "session_id": new_cookie.clone(),
                    "credential_generation": command.credential.credential_generation,
                    "epoch": command.fence.policy.epoch,
                    "audit_id": audit_id,
                    "request_id": command.request.request_id.to_string(),
                })),
                13,
            )
            .await
            .map_err(
                |error| match thrown(&error, &["session_invalid", "policy_stale"]) {
                    Some(_) => LocalAdminError::Unauthenticated,
                    None => infra(error),
                },
            )?;

        let row = rows.into_iter().next().ok_or_else(|| {
            infra(MemoryError::Storage(
                "rotate_session returned no principal".into(),
            ))
        })?;
        let admin_id = require_str(&row, "admin_id")?;
        let credential_generation = require_u64(&row, "credential_generation")?;

        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id,
                session_id: new_cookie,
                credential_generation,
                policy: command.fence.policy,
            },
            username: require_str(&row, "username")?,
            auth_time: parse_datetime(&row, "auth_time")?,
            absolute_expiry: parse_datetime(&row, "absolute_expiry")?,
        })
    }

    /// No result is consumed. Conditional on the presented session id,
    /// admin and credential generation; an unknown session is an error.
    async fn revoke_session(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
    ) -> LocalResult<()> {
        let audit_id = row_id("aud");

        let sql = "
            BEGIN TRANSACTION;
            LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };
            LET $session_row = (SELECT id FROM local_admin_session WHERE cookie_verifier = $session_verifier AND admin_id = $admin_id AND credential_generation = $generation AND revoked_at IS NONE LIMIT 1);
            IF array::len($session_row) = 0 { THROW 'session_not_found'; };
            UPDATE $session_row[0].id SET revoked_at = time::now();
            CREATE type::record('local_admin_audit', $audit_id) SET
                id = $audit_id,
                event_time = time::now(),
                actor_kind = 'admin',
                actor_id = $admin_id,
                action = 'logout',
                target_admin_id = $admin_id,
                target_account_id = NONE,
                target_tenant_id = NONE,
                target_key_id = NONE,
                outcome = 'success',
                reason = NONE,
                request_id = $request_id,
                grants_client_data_access = NONE;
            COMMIT TRANSACTION;
        ";

        self.admin_query(
            sql,
            Some(json!({
                "session_verifier": fence.session_id.clone(),
                "admin_id": fence.admin_id.clone(),
                "generation": fence.credential_generation,
                "epoch": fence.policy.epoch,
                "audit_id": audit_id,
                "request_id": request.request_id.to_string(),
            })),
        )
        .await
        .map_err(
            |error| match thrown(&error, &["session_not_found", "policy_stale"]) {
                Some(_) => LocalAdminError::Unauthenticated,
                None => infra(error),
            },
        )?;

        Ok(())
    }

    /// Statement order: `BEGIN`(0) `LET`(1, policy) `IF`(2, policy)
    /// `LET`(3) `IF`(4..7, four primaries) `LET`(8) `IF`(9..12, four
    /// secondaries) `RETURN`(13) `COMMIT`(14). Result index 13 returns both
    /// bucket rows as one object, so saturation is decided from persisted
    /// counters rather than assumed.
    ///
    /// One bounded row per dimension slot and window; a saturated bucket
    /// increments its saturating counter instead of creating rows. Expired
    /// windows reset against database time.
    async fn reserve_attempt(&self, input: AttemptInput) -> LocalResult<AttemptDecision> {
        if input.source_bucket > MAX_BUCKET {
            return Err(LocalAdminError::InvalidInput(
                "source bucket is out of range".into(),
            ));
        }

        let (primary, secondary) = match input.domain {
            AttemptDomain::Credentials => {
                let username_bucket = input.username_bucket.ok_or_else(|| {
                    LocalAdminError::InvalidInput(
                        "credentials attempts require a username bucket".into(),
                    )
                })?;
                if username_bucket > MAX_BUCKET {
                    return Err(LocalAdminError::InvalidInput(
                        "username bucket is out of range".into(),
                    ));
                }
                (
                    BucketSpec {
                        id: bucket_id("credentials", "username", username_bucket),
                        source: input.source_bucket,
                        username: Some(username_bucket),
                        action: "credentials",
                        window: CREDENTIAL_USERNAME_WINDOW,
                        cap: CREDENTIAL_USERNAME_CAP,
                    },
                    Some(BucketSpec {
                        id: bucket_id("credentials", "source", input.source_bucket),
                        source: input.source_bucket,
                        username: None,
                        action: "credentials",
                        window: CREDENTIAL_SOURCE_WINDOW,
                        cap: CREDENTIAL_SOURCE_CAP,
                    }),
                )
            }
            AttemptDomain::Challenge => (
                BucketSpec {
                    id: bucket_id("challenge", "source", input.source_bucket),
                    source: input.source_bucket,
                    username: None,
                    action: "challenge",
                    window: CHALLENGE_SOURCE_WINDOW,
                    cap: CHALLENGE_SOURCE_CAP,
                },
                None,
            ),
        };
        let secondary = secondary.unwrap_or(BucketSpec {
            id: String::new(),
            source: input.source_bucket,
            username: None,
            action: "challenge",
            window: CHALLENGE_SOURCE_WINDOW,
            cap: CHALLENGE_SOURCE_CAP,
        });
        let has_b = !secondary.id.is_empty();
        let challenge_domain = primary.action == "challenge";

        let sql = "
            BEGIN TRANSACTION;
            LET $guard_policy = (SELECT epoch, mode, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($guard_policy) != 1 OR NOT ('local' IN $guard_policy[0].methods ?? [$guard_policy[0].mode]) OR $guard_policy[0].epoch != $epoch { THROW 'policy_stale'; };
            LET $a = (SELECT denied_count, expires_at FROM local_admin_rate_bucket WHERE bucket_id = $a_id LIMIT 1);
            IF array::len($a) = 0 {
                CREATE type::record('local_admin_rate_bucket', $a_id) SET
                    bucket_id = $a_id,
                    source_bucket = $a_source,
                    username_bucket = IF $a_has_username { $a_username } ELSE { NONE },
                    action = $a_action,
                    reason = NONE,
                    denied_count = 1,
                    window_start = time::now(),
                    expires_at = time::now() + type::duration($a_window);
            };
            IF array::len($a) > 0 AND $a[0].expires_at <= time::now() {
                UPDATE type::record('local_admin_rate_bucket', $a_id) SET
                    denied_count = 1,
                    window_start = time::now(),
                    expires_at = time::now() + type::duration($a_window);
            };
            IF array::len($a) > 0 AND $a[0].expires_at > time::now() AND $a[0].denied_count < $a_cap {
                UPDATE type::record('local_admin_rate_bucket', $a_id) SET denied_count = denied_count + 1;
            };
            IF array::len($a) > 0 AND $a[0].expires_at > time::now() AND $a[0].denied_count >= $a_cap {
                UPDATE type::record('local_admin_rate_bucket', $a_id) SET denied_count = $a_cap + 1;
            };
            LET $b = (SELECT denied_count, expires_at FROM local_admin_rate_bucket WHERE bucket_id = $b_id LIMIT 1);
            IF $has_b AND array::len($b) = 0 {
                CREATE type::record('local_admin_rate_bucket', $b_id) SET
                    bucket_id = $b_id,
                    source_bucket = $b_source,
                    username_bucket = NONE,
                    action = $b_action,
                    reason = NONE,
                    denied_count = 1,
                    window_start = time::now(),
                    expires_at = time::now() + type::duration($b_window);
            };
            IF $has_b AND array::len($b) > 0 AND $b[0].expires_at <= time::now() {
                UPDATE type::record('local_admin_rate_bucket', $b_id) SET
                    denied_count = 1,
                    window_start = time::now(),
                    expires_at = time::now() + type::duration($b_window);
            };
            IF $has_b AND array::len($b) > 0 AND $b[0].expires_at > time::now() AND $b[0].denied_count < $b_cap {
                UPDATE type::record('local_admin_rate_bucket', $b_id) SET denied_count = denied_count + 1;
            };
            IF $has_b AND array::len($b) > 0 AND $b[0].expires_at > time::now() AND $b[0].denied_count >= $b_cap {
                UPDATE type::record('local_admin_rate_bucket', $b_id) SET denied_count = $b_cap + 1;
            };
            RETURN {
                a: (SELECT denied_count, expires_at FROM local_admin_rate_bucket WHERE bucket_id = $a_id LIMIT 1)[0],
                b: (SELECT denied_count, expires_at FROM local_admin_rate_bucket WHERE bucket_id = $b_id LIMIT 1)[0]
            };
            COMMIT TRANSACTION;
        ";

        let vars = json!({
            "epoch": input.policy.epoch,
            "a_id": primary.id,
            "a_source": primary.source,
            "a_username": primary.username.unwrap_or(0),
            "a_has_username": primary.username.is_some(),
            "a_action": primary.action,
            "a_window": primary.window,
            "a_cap": primary.cap,
            "b_id": secondary.id,
            "b_source": secondary.source,
            "b_action": secondary.action,
            "b_window": secondary.window,
            "b_cap": secondary.cap,
            "has_b": has_b,
        });

        // Every attempt in one window touches the same bucket rows, so
        // genuinely concurrent admissions (two logins at once, or every
        // client behind one proxy sharing the source bucket) can lose the
        // write race; the shared execution seam retries that boundedly.
        let rows = self
            .admin_query_at(sql, Some(vars.clone()), 13)
            .await
            .map_err(|error| match thrown(&error, &["policy_stale"]) {
                // A stale fence refuses admission. Challenge flows keep their
                // uniform `400 invalid_challenge` answer so the rejection
                // cannot be told apart from a bad code; credential flows get
                // the uniform 401.
                Some(_) if challenge_domain => LocalAdminError::InvalidChallenge,
                Some(_) => LocalAdminError::Unauthenticated,
                None => infra(error),
            })?;

        let row = rows.into_iter().next().ok_or_else(|| {
            infra(MemoryError::Storage(
                "reserve_attempt returned no bucket state".into(),
            ))
        })?;
        let now = Utc::now();
        let primary_row = row
            .get("a")
            .filter(|value| !value.is_null())
            .ok_or_else(|| {
                infra(MemoryError::Storage(
                    "reserve_attempt persisted no primary bucket".into(),
                ))
            })?;
        let mut retry_after_seconds = saturated_retry(primary_row, primary.cap, now)?;
        if has_b
            && let Some(secondary_row) = row.get("b").filter(|value| !value.is_null())
            && let Some(seconds) = saturated_retry(secondary_row, secondary.cap, now)?
        {
            retry_after_seconds =
                Some(retry_after_seconds.map_or(seconds, |current| current.max(seconds)));
        }

        Ok(match retry_after_seconds {
            Some(retry_after_seconds) => AttemptDecision::Limited {
                retry_after_seconds,
            },
            None => AttemptDecision::Allowed,
        })
    }

    /// No result is consumed. Written after the caller rolls back, so it is
    /// its own transaction; one deduplicated, allowlisted failure event per
    /// request/action pair.
    async fn record_failure(&self, event: FailureAudit) -> LocalResult<()> {
        let action = failure_action_token(event.action);
        let reason = failure_reason_token(event.reason);
        let (actor_kind, actor_id) = match &event.admin_id {
            Some(admin_id) => ("admin", admin_id.clone()),
            None => ("anonymous", "anonymous".to_string()),
        };
        let audit_id = row_id("aud");

        let sql = "
            BEGIN TRANSACTION;
            LET $existing = (SELECT id FROM local_admin_audit WHERE request_id = $request_id AND action = $action LIMIT 1);
            IF array::len($existing) = 0 {
                CREATE type::record('local_admin_audit', $audit_id) SET
                    id = $audit_id,
                    event_time = time::now(),
                    actor_kind = $actor_kind,
                    actor_id = $actor_id,
                    action = $action,
                    target_admin_id = IF $admin_id = '' { NONE } ELSE { $admin_id },
                    target_account_id = NONE,
                    target_tenant_id = NONE,
                    target_key_id = NONE,
                    outcome = 'failure',
                    reason = $reason,
                    request_id = $request_id,
                    grants_client_data_access = NONE;
            };
            COMMIT TRANSACTION;
        ";

        self.admin_query(
            sql,
            Some(json!({
                "request_id": event.request.request_id.to_string(),
                "action": action,
                "reason": reason,
                "actor_kind": actor_kind,
                "actor_id": actor_id,
                "admin_id": event.admin_id.unwrap_or_default(),
                "audit_id": audit_id,
            })),
        )
        .await
        .map_err(infra)?;

        Ok(())
    }

    /// Bounded maintenance pass for the throttle table (spec §7: "expired
    /// counters reset by DB time; cleanup is bounded"). The transaction
    /// shape lives in `local_admin_rate`.
    async fn cleanup_rate_buckets(&self) -> LocalResult<u64> {
        self.cleanup_expired_rate_buckets().await
    }

    /// Guard statements occupy indices 1..=8 (`BEGIN` is 0). Tail:
    /// `LET`(9) `IF`(10) `IF`(11) `LET`(12) `SELECT`(13) `COMMIT`(14).
    /// Result index 13 is the sidecar readback — for a replay it is the
    /// existing resource's current view, for a fresh operation the resource
    /// just written.
    async fn create_client(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        bundle: ClientBundle,
    ) -> LocalResult<ClientView> {
        let account_id = bundle.account.id.clone();
        let tenant_id = bundle.tenant.id.clone();
        let operation_id = bundle.operation_id.to_string();
        let operation_record_id = row_id("op");
        let audit_id = row_id("aud");
        let account_status = encode(&bundle.account.status, "account status")?;
        let tenant_status = encode(&bundle.tenant.status, "tenant status")?;
        let fingerprint = hex::encode(bundle.request_fingerprint);

        let sql = String::from("BEGIN TRANSACTION;")
            + MUTATION_GUARD
            + r#"
            LET $op = (SELECT request_fingerprint, result_resource_id FROM local_admin_operation WHERE admin_id = $admin_id AND operation_kind = 'client_create' AND idempotency_id = $operation_id LIMIT 1);
            IF array::len($op) > 0 AND $op[0].request_fingerprint != $fingerprint { THROW 'idempotency_conflict'; };
            IF array::len($op) = 0 {
                CREATE type::record('account', $account_id) SET
                    id = $account_id,
                    status = $account_status,
                    tenant_id = $tenant_id,
                    created_at = time::now();
                CREATE type::record('tenant', $tenant_id) SET
                    id = $tenant_id,
                    status = $tenant_status,
                    namespace_binding = $namespace_binding,
                    plan_version = $plan_version,
                    schema_version = $schema_version,
                    retry_stage = NONE,
                    provisioning_lease = NONE,
                    created_at = time::now(),
                    version = $tenant_version;
                CREATE type::record('local_admin_client', $account_id) SET
                    account_id = $account_id,
                    display_name = $display_name,
                    creating_admin_id = $admin_id,
                    operation_id = $operation_id,
                    account_status = $account_status,
                    tenant_status = $tenant_status,
                    plan_version = $plan_version,
                    schema_version = $schema_version,
                    version = 1,
                    suspended_from_ready = false,
                    created_at = time::now(),
                    updated_at = time::now();
                CREATE type::record('local_admin_operation', $operation_record_id) SET
                    id = $operation_record_id,
                    admin_id = $admin_id,
                    operation_kind = 'client_create',
                    idempotency_id = $operation_id,
                    request_fingerprint = $fingerprint,
                    result_resource_id = $account_id,
                    committed_at = time::now();
                CREATE type::record('local_admin_audit', $audit_id) SET
                    id = $audit_id,
                    event_time = time::now(),
                    actor_kind = 'admin',
                    actor_id = $admin_id,
                    action = 'client_created',
                    target_admin_id = $admin_id,
                    target_account_id = $account_id,
                    target_tenant_id = $tenant_id,
                    target_key_id = NONE,
                    outcome = 'success',
                    reason = NONE,
                    request_id = $request_id,
                    grants_client_data_access = NONE;
            };
            LET $target = IF array::len($op) > 0 { $op[0].result_resource_id } ELSE { $account_id };
            SELECT account_id, display_name, account_status, tenant_status, plan_version, schema_version, version,
                   (SELECT VALUE tenant_id FROM account WHERE id = type::record('account', $parent.account_id) LIMIT 1)[0] AS tenant_id
            FROM local_admin_client WHERE account_id = $target LIMIT 1;
            COMMIT TRANSACTION;
        "#;

        let rows = self
            .admin_query_at(
                &sql,
                Some(with_guard(
                    fence,
                    json!({
                        "account_id": account_id,
                        "tenant_id": tenant_id,
                        "display_name": bundle.display_name,
                        "operation_id": operation_id,
                        "operation_record_id": operation_record_id,
                        "fingerprint": fingerprint,
                        "account_status": account_status.clone(),
                        "tenant_status": tenant_status.clone(),
                        "namespace_binding": bundle.tenant.namespace_binding,
                        "plan_version": bundle.tenant.plan_version,
                        "schema_version": bundle.tenant.schema_version,
                        "tenant_version": bundle.tenant.version,
                        "audit_id": audit_id,
                        "request_id": request.request_id.to_string(),
                    }),
                )),
                13,
            )
            .await
            .map_err(|error| {
                match thrown(
                    &error,
                    &[
                        "policy_stale",
                        "admin_stale",
                        "session_invalid",
                        "idempotency_conflict",
                    ],
                ) {
                    Some("idempotency_conflict") => LocalAdminError::IdempotencyConflict,
                    Some(_) => LocalAdminError::Unauthenticated,
                    None => infra(error),
                }
            })?;

        let row = rows.into_iter().next().ok_or_else(|| {
            infra(MemoryError::Storage(
                "create_client returned no client row".into(),
            ))
        })?;
        decode_client_view(&row)
    }

    /// Result index 0 is the `{ items: [...] }` envelope.
    ///
    /// The list is **not** scoped to the creating administrator: plan R2
    /// requires that equal local administrators see the same clients, and the
    /// sidecar keeps `creating_admin_id` as an audit attribute rather than an
    /// access boundary.
    async fn list_clients(
        &self,
        _fence: &AdminFence,
        page: PageRequest,
    ) -> LocalResult<Page<ClientView>> {
        let limit = page.limit.clamp(1, 100);
        let after = page.after.unwrap_or_default();

        let sql = format!(
            "
            RETURN {{ items: (SELECT account_id, display_name, account_status, tenant_status, plan_version, schema_version, version,
                   {CLIENT_LIVE_PROJECTION}
            FROM local_admin_client
            WHERE account_id > $cursor
            ORDER BY account_id ASC
            LIMIT $limit) }};
        "
        );

        let rows = self
            .admin_query_at(
                &sql,
                Some(json!({
                    "cursor": after,
                    "limit": limit,
                })),
                0,
            )
            .await
            .map_err(infra)?;

        let items = decode_page(rows.first(), decode_client_view)?;
        let next_cursor = if items.len() == usize::from(limit) {
            items.last().map(|client| client.account_id.clone())
        } else {
            None
        };

        Ok(Page { items, next_cursor })
    }

    /// Result index 0 is the single sidecar row, or `NotFound`. `query_json`
    /// maps a zero-row result to an empty vector rather than an error.
    ///
    /// Readable by any local administrator (plan R2).
    async fn client(&self, _fence: &AdminFence, account_id: &str) -> LocalResult<ClientView> {
        let sql = format!(
            "
            SELECT account_id, display_name, account_status, tenant_status, plan_version, schema_version, version,
                   {CLIENT_LIVE_PROJECTION}
            FROM local_admin_client
            WHERE account_id = $account_id
            LIMIT 1;
        "
        );

        let rows = self
            .admin_query(&sql, Some(json!({"account_id": account_id})))
            .await
            .map_err(infra)?;

        let row = rows.into_iter().next().ok_or(LocalAdminError::NotFound)?;
        decode_client_view(&row)
    }

    /// The client sidecar is checked first (result index 0 of the guard
    /// read) so a non-local account is `NotFound`; the page itself is read
    /// from the **authoritative** `api_key` table, which is where
    /// `last_used_at` and `status` live (spec §8). The page uses the stable
    /// `key_id` cursor, and any local administrator may read it (plan R2).
    async fn list_client_keys(
        &self,
        _fence: &AdminFence,
        account_id: &str,
        page: PageRequest,
    ) -> LocalResult<Page<ApiKeyMeta>> {
        let limit = page.limit.clamp(1, 100);
        let after = page.after.unwrap_or_default();

        let owner = self
            .admin_query(
                "SELECT account_id FROM local_admin_client WHERE account_id = $account_id LIMIT 1;",
                Some(json!({"account_id": account_id})),
            )
            .await
            .map_err(infra)?;
        if owner.is_empty() {
            return Err(LocalAdminError::NotFound);
        }

        let sql = "
            RETURN { items: (SELECT id, account_id, name, verifier, status, created_at, expires_at, last_used_at, version
            FROM api_key
            WHERE account_id = $account_id AND id > $cursor
            ORDER BY id ASC
            LIMIT $limit) };
        ";

        let rows = self
            .admin_query_at(
                sql,
                Some(json!({
                    "account_id": account_id,
                    "cursor": after,
                    "limit": limit,
                })),
                0,
            )
            .await
            .map_err(infra)?;

        let items = decode_page(rows.first(), key_meta_from_api_key)?;
        let next_cursor = if items.len() == usize::from(limit) {
            items.last().map(|key| key.id.clone())
        } else {
            None
        };

        Ok(Page { items, next_cursor })
    }

    /// Guard statements occupy indices 1..=8 (`BEGIN` is 0). Tail:
    /// `LET`(9) `IF`(10) `LET`(11) `IF`(12) `IF`(13) `LET`(14)
    /// `SELECT`(15) `COMMIT`(16). Result index 15 is the key readback —
    /// on a replay the original key, otherwise the key just written.
    ///
    /// The key is persisted in **two** tables in the same transaction:
    /// `api_key` is the single durable source of truth for data-plane
    /// bearer authorization (`RegistryStore::find_api_key`) and for key
    /// metadata (this method reads it back from there), and
    /// `local_admin_client_key` is the local-workflow ledger the admin
    /// surface scopes revocation by. Because both rows commit together
    /// (and revocation updates both together), the ledger cannot drift from
    /// the authoritative authorization row.
    async fn insert_client_key(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        command: AdminKeyInsert,
    ) -> LocalResult<KeyInsertOutcome> {
        let operation_id = command.operation_id.to_string();
        let operation_record_id = row_id("op");
        let audit_id = row_id("aud");
        let fingerprint = hex::encode(command.request_fingerprint);
        let (expiry_never, expiry_window) = match &command.expiry {
            KeyExpiry::Never => (true, "0s".to_string()),
            KeyExpiry::Days { days } => (false, format!("{days}d")),
        };

        let sql = String::from("BEGIN TRANSACTION;")
            + MUTATION_GUARD
            + r#"
            LET $client = (SELECT plan_version FROM local_admin_client WHERE account_id = $account_id LIMIT 1);
            IF array::len($client) = 0 { THROW 'not_found'; };
            LET $op = (SELECT request_fingerprint, result_resource_id FROM local_admin_operation WHERE admin_id = $admin_id AND operation_kind = 'key_issue' AND idempotency_id = $operation_id LIMIT 1);
            IF array::len($op) > 0 AND $op[0].request_fingerprint != $fingerprint { THROW 'idempotency_conflict'; };
            IF array::len($op) = 0 {
                LET $account = (SELECT tenant_id, status FROM account WHERE id = type::record('account', $account_id) LIMIT 1);
                IF array::len($account) = 0 { THROW 'state_conflict'; };
                IF $account[0].status != 'active' { THROW 'state_conflict'; };
                LET $tenant = (SELECT status FROM tenant WHERE id = type::record('tenant', $account[0].tenant_id) LIMIT 1);
                IF array::len($tenant) = 0 { THROW 'state_conflict'; };
                IF $tenant[0].status != 'ready' { THROW 'state_conflict'; };
                LET $plan = (SELECT limits.max_active_api_keys AS cap FROM plan WHERE version = $client[0].plan_version LIMIT 1);
                IF array::len($plan) = 0 { THROW 'plan_missing'; };
                UPDATE type::record('local_admin_client', $account_id) SET version = version + 1, updated_at = time::now();
                LET $count = (SELECT count() AS count FROM api_key WHERE account_id = $account_id AND status = 'active' AND (expires_at IS NONE OR expires_at > time::now()) GROUP ALL);
                IF array::len($count) > 0 AND $count[0].count >= $plan[0].cap { THROW 'key_cap'; };
                LET $expires_at = IF $expiry_never { NONE } ELSE { time::now() + type::duration($expiry_window) };
                CREATE type::record('api_key', $key_id) SET
                    id = $key_id,
                    account_id = $account_id,
                    name = $name,
                    verifier = $verifier,
                    status = 'active',
                    created_at = time::now(),
                    expires_at = $expires_at,
                    last_used_at = NONE,
                    version = 1;
                CREATE type::record('local_admin_client_key', $key_id) SET
                    key_id = $key_id,
                    account_id = $account_id,
                    name = $name,
                    verifier = $verifier,
                    created_at = time::now(),
                    expires_at = $expires_at,
                    revoked = false;
                CREATE type::record('local_admin_operation', $operation_record_id) SET
                    id = $operation_record_id,
                    admin_id = $admin_id,
                    operation_kind = 'key_issue',
                    idempotency_id = $operation_id,
                    request_fingerprint = $fingerprint,
                    result_resource_id = $key_id,
                    committed_at = time::now();
                CREATE type::record('local_admin_audit', $audit_id) SET
                    id = $audit_id,
                    event_time = time::now(),
                    actor_kind = 'admin',
                    actor_id = $admin_id,
                    action = 'key_issued',
                    target_admin_id = $admin_id,
                    target_account_id = $account_id,
                    target_tenant_id = $account[0].tenant_id,
                    target_key_id = $key_id,
                    outcome = 'success',
                    reason = NONE,
                    request_id = $request_id,
                    grants_client_data_access = true;
            };
            LET $target = IF array::len($op) > 0 { $op[0].result_resource_id } ELSE { $key_id };
            SELECT id, account_id, name, verifier, status, created_at, expires_at, last_used_at, version FROM api_key WHERE id = type::record('api_key', $target) LIMIT 1;
            COMMIT TRANSACTION;
        "#;

        let rows = self
            .admin_query_at(
                &sql,
                Some(with_guard(
                    fence,
                    json!({
                        "account_id": command.account_id,
                        "key_id": command.key_id.clone(),
                        "name": command.name,
                        "verifier": hex::encode(command.verifier.0),
                        "operation_id": operation_id,
                        "operation_record_id": operation_record_id,
                        "fingerprint": fingerprint,
                        "expiry_never": expiry_never,
                        "expiry_window": expiry_window,
                        "audit_id": audit_id,
                        "request_id": request.request_id.to_string(),
                    }),
                )),
                15,
            )
            .await
            .map_err(|error| {
                match thrown(
                    &error,
                    &[
                        "policy_stale",
                        "admin_stale",
                        "session_invalid",
                        "not_found",
                        "idempotency_conflict",
                        "state_conflict",
                        "plan_missing",
                        "key_cap",
                    ],
                ) {
                    Some("not_found") => LocalAdminError::NotFound,
                    Some("idempotency_conflict") => LocalAdminError::IdempotencyConflict,
                    Some("state_conflict") => LocalAdminError::StateConflict,
                    Some("key_cap") => LocalAdminError::KeyCap,
                    Some(_) => LocalAdminError::Unauthenticated,
                    None => infra(error),
                }
            })?;

        let row = rows.into_iter().next().ok_or_else(|| {
            infra(MemoryError::Storage(
                "insert_client_key returned no key row".into(),
            ))
        })?;
        let stored_key_id = decode_record_id(&row, "");
        if stored_key_id != command.key_id {
            return Ok(KeyInsertOutcome::AlreadyIssued {
                key_id: stored_key_id,
            });
        }
        Ok(KeyInsertOutcome::Created(key_meta_from_api_key(&row)?))
    }

    /// No result is consumed. A non-existent or foreign-owner key is
    /// `NotFound`; an already-revoked owned key is a no-op; the first
    /// transition appends the audit.
    async fn revoke_client_key(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        key_id: &str,
    ) -> LocalResult<()> {
        let audit_id = row_id("aud");

        let sql = String::from("BEGIN TRANSACTION;")
            + MUTATION_GUARD
            + r#"
            LET $client = (SELECT account_id FROM local_admin_client WHERE account_id = $account_id LIMIT 1);
            IF array::len($client) = 0 { THROW 'key_not_found'; };
            LET $key = (SELECT revoked FROM local_admin_client_key WHERE key_id = $key_id AND account_id = $account_id LIMIT 1);
            IF array::len($key) = 0 { THROW 'key_not_found'; };
            IF $key[0].revoked = false {
                UPDATE type::record('local_admin_client_key', $key_id) SET revoked = true;
                UPDATE type::record('api_key', $key_id) SET status = 'revoked', version = version + 1;
                UPDATE type::record('local_admin_client', $account_id) SET version = version + 1, updated_at = time::now();
                CREATE type::record('local_admin_audit', $audit_id) SET
                    id = $audit_id,
                    event_time = time::now(),
                    actor_kind = 'admin',
                    actor_id = $admin_id,
                    action = 'key_revoked',
                    target_admin_id = $admin_id,
                    target_account_id = $account_id,
                    target_tenant_id = NONE,
                    target_key_id = $key_id,
                    outcome = 'success',
                    reason = NONE,
                    request_id = $request_id,
                    grants_client_data_access = NONE;
            };
            COMMIT TRANSACTION;
        "#;

        self.admin_query(
            &sql,
            Some(with_guard(
                fence,
                json!({
                    "account_id": account_id,
                    "key_id": key_id,
                    "audit_id": audit_id,
                    "request_id": request.request_id.to_string(),
                }),
            )),
        )
        .await
        .map_err(|error| {
            match thrown(
                &error,
                &[
                    "policy_stale",
                    "admin_stale",
                    "session_invalid",
                    "key_not_found",
                ],
            ) {
                Some("key_not_found") => LocalAdminError::NotFound,
                Some(_) => LocalAdminError::Unauthenticated,
                None => infra(error),
            }
        })?;

        Ok(())
    }

    /// No result is consumed. A request already at the coherent desired
    /// state is a no-op with no writes and no audit, even when
    /// `expected_version` is stale; otherwise a stale version conflicts.
    /// Incoherent pairs and Reserved/Migrating/Failed/Deleting/Purged
    /// tenants are `StateConflict`.
    async fn set_client_state(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        expected_version: u64,
        action: ClientStateAction,
    ) -> LocalResult<()> {
        let (
            source_account,
            source_tenant,
            source_marker,
            desired_account,
            desired_tenant,
            desired_marker,
            audit_action,
        ) = match action {
            ClientStateAction::Suspend => (
                "active",
                "ready",
                false,
                "suspended",
                "suspended",
                true,
                "client_suspended",
            ),
            ClientStateAction::Resume => (
                "suspended",
                "suspended",
                true,
                "active",
                "ready",
                false,
                "client_resumed",
            ),
        };
        let audit_id = row_id("aud");

        let sql = String::from("BEGIN TRANSACTION;")
            + MUTATION_GUARD
            + r#"
            LET $client = (SELECT version, suspended_from_ready FROM local_admin_client WHERE account_id = $account_id LIMIT 1);
            IF array::len($client) = 0 { THROW 'not_found'; };
            LET $account = (SELECT tenant_id, status FROM account WHERE id = type::record('account', $account_id) LIMIT 1);
            IF array::len($account) = 0 { THROW 'state_conflict'; };
            LET $tenant = (SELECT status FROM tenant WHERE id = type::record('tenant', $account[0].tenant_id) LIMIT 1);
            IF array::len($tenant) = 0 { THROW 'state_conflict'; };
            LET $current = { account_status: $account[0].status, tenant_status: $tenant[0].status, suspended_from_ready: $client[0].suspended_from_ready, version: $client[0].version };
            LET $is_noop = ($current.account_status = $desired_account AND $current.tenant_status = $desired_tenant AND $current.suspended_from_ready = $desired_marker);
            IF $is_noop = false AND ($current.account_status != $source_account OR $current.tenant_status != $source_tenant OR $current.suspended_from_ready != $source_marker) {
                THROW 'state_conflict';
            };
            IF $is_noop = false AND $current.version != $expected_version { THROW 'version_conflict'; };
            IF $is_noop = false {
                UPDATE local_admin_client SET
                    account_status = $desired_account,
                    tenant_status = $desired_tenant,
                    suspended_from_ready = $desired_marker,
                    version = version + 1,
                    updated_at = time::now()
                    WHERE account_id = $account_id;
                UPDATE type::record('account', $account_id) SET status = $desired_account;
                UPDATE type::record('tenant', $account[0].tenant_id) SET status = $desired_tenant, version = version + 1;
                CREATE type::record('local_admin_audit', $audit_id) SET
                    id = $audit_id,
                    event_time = time::now(),
                    actor_kind = 'admin',
                    actor_id = $admin_id,
                    action = $audit_action,
                    target_admin_id = $admin_id,
                    target_account_id = $account_id,
                    target_tenant_id = $account[0].tenant_id,
                    target_key_id = NONE,
                    outcome = 'success',
                    reason = NONE,
                    request_id = $request_id,
                    grants_client_data_access = NONE;
            };
            COMMIT TRANSACTION;
        "#;

        self.admin_query(
            &sql,
            Some(with_guard(
                fence,
                json!({
                    "account_id": account_id,
                    "expected_version": expected_version,
                    "source_account": source_account,
                    "source_tenant": source_tenant,
                    "source_marker": source_marker,
                    "desired_account": desired_account,
                    "desired_tenant": desired_tenant,
                    "desired_marker": desired_marker,
                    "audit_action": audit_action,
                    "audit_id": audit_id,
                    "request_id": request.request_id.to_string(),
                }),
            )),
        )
        .await
        .map_err(|error| {
            match thrown(
                &error,
                &[
                    "policy_stale",
                    "admin_stale",
                    "session_invalid",
                    "not_found",
                    "state_conflict",
                    "version_conflict",
                ],
            ) {
                Some("not_found") => LocalAdminError::NotFound,
                Some("state_conflict") => LocalAdminError::StateConflict,
                Some("version_conflict") => LocalAdminError::VersionConflict,
                Some(_) => LocalAdminError::Unauthenticated,
                None => infra(error),
            }
        })?;

        Ok(())
    }
}

/// One throttle dimension slot for a single reservation.
struct BucketSpec {
    id: String,
    source: u16,
    username: Option<u16>,
    action: &'static str,
    window: &'static str,
    cap: u64,
}

/// Decode an issue-key row into public metadata.
fn key_meta_from_api_key(row: &Value) -> LocalResult<ApiKeyMeta> {
    let key = super::decode_api_key(row).map_err(infra)?;
    Ok(ApiKeyMeta {
        id: key.id,
        name: key.name,
        status: key.status,
        created_at: key.created_at,
        expires_at: key.expires_at,
        last_used_at: key.last_used_at,
    })
}

/// Decode a `RETURN { items: (SELECT …) }` envelope into a page of rows.
///
/// The durable handle can only take a single value per statement, so
/// multi-row reads are wrapped in one object; a missing or malformed
/// envelope is an error, never an empty page.
fn decode_page<T>(
    row: Option<&Value>,
    decode: impl Fn(&Value) -> LocalResult<T>,
) -> LocalResult<Vec<T>> {
    let row = row.ok_or_else(|| {
        infra(MemoryError::Storage(
            "list query returned no envelope".into(),
        ))
    })?;
    let items = row
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| infra(MemoryError::Storage("list envelope has no items".into())))?;
    items.iter().map(decode).collect()
}

/// Seconds until the bucket window reopens when the persisted counter is
/// over its cap; `None` means the attempt is admitted.
fn saturated_retry(row: &Value, cap: u64, now: chrono::DateTime<Utc>) -> LocalResult<Option<u32>> {
    if require_u64(row, "denied_count")? <= cap {
        return Ok(None);
    }
    let expires_at = parse_datetime(row, "expires_at")?;
    let seconds = expires_at.signed_duration_since(now).num_seconds().max(0);
    Ok(Some(u32::try_from(seconds).unwrap_or(u32::MAX)))
}

#[cfg(test)]
mod sql_fault_tests {
    //! Plan §5 experiments that need a *statement* to fail.
    //!
    //! Each case arms [`crate::http::fault_injection::SqlFaultHook`] with a
    //! needle naming a local-admin statement, drives the real service over
    //! the real durable store, and asserts both the surfaced error and the
    //! absence of any partial credential, session, client or state change.

    use super::*;
    use crate::service::local_admin::auth::{
        AdminManagementService, LocalAdminAuthority, LocalAdminService,
    };
    use crate::service::local_admin::contracts::{
        AuthAttemptContext, ChallengeKind, RequestContext,
    };
    use crate::service::local_admin::password::PasswordHasher;
    use std::sync::Arc;

    const PASSWORD: &str = "SecureP@ssw0rd123";

    pub(super) fn attempt() -> AuthAttemptContext {
        AuthAttemptContext {
            request: RequestContext {
                request_id: uuid::Uuid::new_v4(),
            },
            source: std::net::IpAddr::from([127, 0, 0, 1]),
        }
    }

    pub(super) fn command_request() -> RequestContext {
        RequestContext {
            request_id: uuid::Uuid::new_v4(),
        }
    }

    pub(super) async fn fixture() -> (
        Arc<SurrealRegistryStore>,
        Arc<LocalAdminAuthority>,
        Arc<LocalAdminService>,
        AdminManagementService,
    ) {
        let namespace = format!("local_admin_sql_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(
            SurrealRegistryStore::connect_in_memory(&namespace, "registry")
                .await
                .expect("migrated Mem registry"),
        );
        let authority =
            LocalAdminAuthority::join_local_for_test(store.clone(), [1u8; 32], [2u8; 32])
                .await
                .expect("join durable local policy");
        let hasher = Arc::new(PasswordHasher::new().expect("supported KDF"));
        let service = Arc::new(LocalAdminService::new(authority.clone(), hasher));
        let management = AdminManagementService::new(authority.clone());
        (store, authority, service, management)
    }

    /// Row count for a local-admin table, read through the same seam the
    /// production statements use.
    async fn count_rows(store: &SurrealRegistryStore, table: &str) -> u64 {
        let rows = store
            .admin_query(&format!("SELECT VALUE id FROM {table};"), None)
            .await
            .expect("count rows");
        rows.len() as u64
    }

    /// Activate an administrator and return their session cookie verifier.
    async fn activate_and_login(
        management: &AdminManagementService,
        service: &LocalAdminService,
    ) -> [u8; 32] {
        let challenge = management
            .create_admin("ops.one", &command_request())
            .await
            .expect("create admin");
        service
            .finish_challenge(
                &attempt(),
                &challenge.code,
                ChallengeKind::Activate,
                PASSWORD.to_owned(),
            )
            .await
            .expect("activate");
        let login = service
            .login(&attempt(), "ops.one", PASSWORD.to_owned())
            .await
            .expect("login");
        let raw = &login.cookie_value;
        hex::decode(raw)
            .expect("hex cookie verifier")
            .try_into()
            .expect("32-byte cookie verifier")
    }

    #[tokio::test]
    async fn reservation_storage_error_fails_closed_without_admitting_the_attempt() {
        // Plan §5: "DB unavailable during reserve". The reservation is the
        // first durable step, so failing it must produce one infrastructure
        // error, no session, and no admitted-failure audit row (the attempt
        // was never reserved).
        let (store, _authority, service, management) = fixture().await;
        let _ = activate_and_login(&management, &service).await;
        let sessions_before = count_rows(&store, "local_admin_session").await;
        // `create_admin`, activation and login each append one success
        // audit row (issue challenge, finish challenge, open session).
        let audits_before = count_rows(&store, "local_admin_audit").await;
        assert_eq!(audits_before, 3, "create + activate + login audit rows");

        store.arm_sql_fault("local_admin_rate_bucket");
        let denied = service
            .login(&attempt(), "ops.one", PASSWORD.to_owned())
            .await;
        assert!(
            matches!(denied, Err(LocalAdminError::Infrastructure(_))),
            "a store outage must surface as infrastructure, not credentials"
        );

        assert_eq!(
            count_rows(&store, "local_admin_session").await,
            sessions_before,
            "a failed reservation must not open a session"
        );
        assert_eq!(
            count_rows(&store, "local_admin_audit").await,
            audits_before,
            "a throttled/never-admitted attempt is counted in the rate bucket, not appended"
        );

        // The service recovers as soon as the store does.
        assert!(
            service
                .login(&attempt(), "ops.one", PASSWORD.to_owned())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn session_insert_statement_error_leaves_no_session_and_rolls_back() {
        // Plan §5: "Inject a nonselected SQL statement error" — the failure
        // lands on the session insert inside `open_session`, which aborts
        // the whole transaction, so no session row and no success audit row
        // can survive, and the admin can still log in afterwards.
        let (store, _authority, service, management) = fixture().await;
        let challenge = management
            .create_admin("ops.one", &command_request())
            .await
            .expect("create admin");
        service
            .finish_challenge(
                &attempt(),
                &challenge.code,
                ChallengeKind::Activate,
                PASSWORD.to_owned(),
            )
            .await
            .expect("activate");
        assert_eq!(count_rows(&store, "local_admin_session").await, 0);
        assert_eq!(
            count_rows(&store, "local_admin_audit").await,
            2,
            "issue challenge + activation audit rows only"
        );

        store.arm_sql_fault("CREATE type::record('local_admin_session'");
        let failed = service
            .login(&attempt(), "ops.one", PASSWORD.to_owned())
            .await;
        assert!(
            matches!(failed, Err(LocalAdminError::Infrastructure(_))),
            "an aborted session insert must not be reported as success"
        );
        assert_eq!(
            count_rows(&store, "local_admin_session").await,
            0,
            "the aborted transaction must leave no session row"
        );
        assert_eq!(
            count_rows(&store, "local_admin_audit").await,
            2,
            "the aborted transaction must leave no success audit row"
        );

        assert!(
            service
                .login(&attempt(), "ops.one", PASSWORD.to_owned())
                .await
                .is_ok(),
            "the next login succeeds once storage recovers"
        );
    }

    #[tokio::test]
    async fn success_audit_error_rolls_back_the_credential_change() {
        // Plan §5: "audit insert error" on the *success* path. The activation
        // transaction appends its audit row after the credential write, so
        // failing it must leave the admin pending and the challenge usable.
        let (store, _authority, service, management) = fixture().await;
        let challenge = management
            .create_admin("ops.one", &command_request())
            .await
            .expect("create admin");

        store.arm_sql_fault("CREATE type::record('local_admin_audit'");
        let failed = service
            .finish_challenge(
                &attempt(),
                &challenge.code,
                ChallengeKind::Activate,
                PASSWORD.to_owned(),
            )
            .await;
        assert!(
            matches!(failed, Err(LocalAdminError::Infrastructure(_))),
            "a failed success-audit write must not report success"
        );
        assert_eq!(
            count_rows(&store, "local_admin_audit").await,
            1,
            "the issue-challenge audit row is the only one present"
        );
        assert!(
            service
                .login(&attempt(), "ops.one", PASSWORD.to_owned())
                .await
                .is_err(),
            "a rolled-back activation leaves no usable password"
        );

        // The same challenge still completes: nothing was half-consumed.
        assert!(
            service
                .finish_challenge(
                    &attempt(),
                    &challenge.code,
                    ChallengeKind::Activate,
                    PASSWORD.to_owned(),
                )
                .await
                .is_ok(),
            "the challenge must survive the rolled-back attempt"
        );
    }

    #[tokio::test]
    async fn failure_audit_storage_error_is_sanitized_unavailable_not_a_rejection() {
        // Plan §5: "audit insert error" on the *failure* path. Spec §10
        // requires a failed-login audit write to fail closed with a
        // sanitized 503 and never to be silently dropped.
        let (store, _authority, service, management) = fixture().await;
        let _ = activate_and_login(&management, &service).await;
        let audits_before = count_rows(&store, "local_admin_audit").await;
        assert_eq!(audits_before, 3, "create + activate + login audit rows");

        store.arm_sql_fault("CREATE type::record('local_admin_audit'");
        let result = service
            .login(&attempt(), "ops.one", "WrongP@ssw0rd123".to_owned())
            .await;
        assert!(
            matches!(result, Err(LocalAdminError::Unavailable)),
            "a lost failure-audit write must not be reported as a credential rejection"
        );
        assert_eq!(
            count_rows(&store, "local_admin_audit").await,
            audits_before,
            "the failure event really was not written"
        );

        // Storage recovers: the rejection is recorded and reported as such.
        assert!(matches!(
            service
                .login(&attempt(), "ops.one", "WrongP@ssw0rd123".to_owned())
                .await,
            Err(LocalAdminError::InvalidCredentials)
        ));
        assert_eq!(
            count_rows(&store, "local_admin_audit").await,
            audits_before + 1,
            "the admitted failure appends exactly one audit row"
        );
    }

    #[tokio::test]
    async fn admitted_failure_appends_one_row_per_request_id() {
        // One deduplicated, allowlisted event per reserved attempt, by
        // request id: repeating the same request must not append again, and
        // a different request must.
        let (store, authority, _service, _management) = fixture().await;
        let shared = RequestContext {
            request_id: uuid::Uuid::new_v4(),
        };
        let event = |reason: FailureReason| FailureAudit {
            request: shared.clone(),
            policy: authority.policy().clone(),
            action: FailureAction::Login,
            reason,
            admin_id: None,
            username_bucket: Some(7),
            source_bucket: Some(11),
            admitted_auth_attempt: true,
        };

        store
            .record_failure(event(FailureReason::InvalidCredentials))
            .await
            .expect("first failure event");
        store
            .record_failure(event(FailureReason::InvalidCredentials))
            .await
            .expect("replayed failure event");
        assert_eq!(
            count_rows(&store, "local_admin_audit").await,
            1,
            "one event per request id"
        );

        let rows = store
            .admin_query(
                "SELECT action, outcome, reason, actor_kind, target_admin_id FROM local_admin_audit;",
                None,
            )
            .await
            .expect("read the audit row");
        let row = rows.first().expect("one audit row");
        assert_eq!(row["action"], serde_json::json!("login"));
        assert_eq!(row["outcome"], serde_json::json!("failure"));
        assert_eq!(row["reason"], serde_json::json!("invalid_credentials"));
        assert_eq!(row["actor_kind"], serde_json::json!("anonymous"));
        assert!(
            row["target_admin_id"].is_null(),
            "an anonymous failure names no admin"
        );
    }

    #[tokio::test]
    async fn failure_audit_accepts_a_stale_epoch_as_an_attribute() {
        // Plan §3.1: a stale supplied epoch is an event attribute, not a
        // reason to reject the record — otherwise every stale-fence
        // rejection would turn into an unrelated audit failure.
        let (store, authority, _service, _management) = fixture().await;
        let stale = BrowserPolicyFence {
            methods: vec![crate::http::config::BrowserAuthMethod::Local],
            epoch: authority.policy().epoch.wrapping_add(41),
        };
        store
            .record_failure(FailureAudit {
                request: RequestContext {
                    request_id: uuid::Uuid::new_v4(),
                },
                policy: stale,
                action: FailureAction::Session,
                reason: FailureReason::StaleFence,
                admin_id: Some("admin_ops".to_owned()),
                username_bucket: None,
                source_bucket: None,
                admitted_auth_attempt: false,
            })
            .await
            .expect("a stale epoch must not fail the audit write");
        assert_eq!(count_rows(&store, "local_admin_audit").await, 1);

        let rows = store
            .admin_query(
                "SELECT action, reason, actor_kind FROM local_admin_audit;",
                None,
            )
            .await
            .expect("read the audit row");
        let row = rows.first().expect("one audit row");
        assert_eq!(row["reason"], serde_json::json!("stale_fence"));
        assert_eq!(
            row["actor_kind"],
            serde_json::json!("admin"),
            "a named actor is recorded as such"
        );
    }
}

#[cfg(test)]
mod guard_semantics_tests {
    //! Review findings on the mutation guard and the durable policy fence.
    //!
    //! The guard case drives the production [`MUTATION_GUARD`] fragment
    //! exactly as the four guarded client mutations do; the fence cases drive
    //! the real service over the real durable store.

    use super::sql_fault_tests::{attempt, command_request, fixture};
    use super::*;
    use crate::service::local_admin::contracts::ChallengeKind;

    const PASSWORD: &str = "SecureP@ssw0rd123";

    #[tokio::test]
    async fn mutation_guard_touches_a_session_in_its_final_thirty_minutes() {
        let (store, _authority, service, management) = fixture().await;
        let challenge = management
            .create_admin("ops.one", &command_request())
            .await
            .expect("create admin");
        service
            .finish_challenge(
                &attempt(),
                &challenge.code,
                ChallengeKind::Activate,
                PASSWORD.to_owned(),
            )
            .await
            .expect("activate");
        let login = service
            .login(&attempt(), "ops.one", PASSWORD.to_owned())
            .await
            .expect("login");

        // Test-only state surgery: put the session inside its final 30
        // minutes of absolute lifetime with a stale idle deadline.
        store
            .admin_query(
                "UPDATE local_admin_session SET absolute_expiry = time::now() + 600s, idle_expiry = time::now() + 60s;",
                None,
            )
            .await
            .expect("shrink the session lifetime");

        // The guard fragment must write the presented session row even here:
        // without a write intent there is no conflict with a concurrent
        // logout or rotation, and a paused mutation could serialize after
        // revocation (spec §7).
        let sql = format!(
            "BEGIN TRANSACTION;{MUTATION_GUARD}
             SELECT id FROM local_admin_session WHERE cookie_verifier = $session_verifier LIMIT 1;
             COMMIT TRANSACTION;"
        );
        store
            .admin_query(&sql, Some(with_guard(&login.principal.fence, json!({}))))
            .await
            .expect("guarded transaction near the absolute deadline");

        let rows = store
            .admin_query(
                "SELECT idle_expiry, absolute_expiry FROM local_admin_session;",
                None,
            )
            .await
            .expect("read deadlines");
        let row = rows.first().expect("one session row");
        let idle = parse_datetime(row, "idle_expiry").expect("idle expiry");
        let absolute = parse_datetime(row, "absolute_expiry").expect("absolute expiry");
        assert_eq!(
            idle, absolute,
            "the guard touches the row and caps the idle deadline at the absolute one"
        );
    }

    #[tokio::test]
    async fn a_policy_epoch_bump_stops_session_resolution_and_minting() {
        let (store, _authority, service, management) = fixture().await;
        let challenge = management
            .create_admin("ops.one", &command_request())
            .await
            .expect("create admin");
        service
            .finish_challenge(
                &attempt(),
                &challenge.code,
                ChallengeKind::Activate,
                PASSWORD.to_owned(),
            )
            .await
            .expect("activate");
        let login = service
            .login(&attempt(), "ops.one", PASSWORD.to_owned())
            .await
            .expect("login");
        let raw = &login.cookie_value;
        let verifier: [u8; 32] = hex::decode(raw)
            .expect("hex cookie verifier")
            .try_into()
            .expect("32-byte cookie verifier");

        // What `admin auth-methods remove` and offline key rotation do to the
        // durable policy: advance the epoch.
        store
            .admin_query(
                "UPDATE browser_auth_policy SET epoch = epoch + 1, version = version + 1;",
                None,
            )
            .await
            .expect("advance the policy epoch");

        assert!(
            matches!(
                service.resolve(&command_request(), &verifier).await,
                Err(LocalAdminError::Unauthenticated)
            ),
            "an old-epoch session must stop resolving the moment the epoch moves"
        );
        assert!(
            matches!(
                service
                    .login(&attempt(), "ops.one", PASSWORD.to_owned())
                    .await,
                Err(LocalAdminError::Unauthenticated)
            ),
            "no session may be minted under the stale fence"
        );
    }

    #[tokio::test]
    async fn a_policy_epoch_bump_rejects_challenge_inspection_and_finish() {
        let (store, _authority, service, management) = fixture().await;
        let challenge = management
            .create_admin("ops.one", &command_request())
            .await
            .expect("create admin");
        store
            .admin_query(
                "UPDATE browser_auth_policy SET epoch = epoch + 1, version = version + 1;",
                None,
            )
            .await
            .expect("advance the policy epoch");
        assert!(
            matches!(
                service
                    .inspect_challenge(&attempt(), &challenge.code, ChallengeKind::Activate)
                    .await,
                Err(LocalAdminError::InvalidChallenge)
            ),
            "challenge inspection refuses the stale fence with the uniform answer"
        );
        assert!(
            matches!(
                service
                    .finish_challenge(
                        &attempt(),
                        &challenge.code,
                        ChallengeKind::Activate,
                        PASSWORD.to_owned(),
                    )
                    .await,
                Err(LocalAdminError::InvalidChallenge)
            ),
            "challenge completion refuses the stale fence too"
        );
    }

    #[tokio::test]
    async fn local_admin_tables_deny_unprivileged_access() {
        // Migration 051 restores the 001 deny-everything permissions on the
        // nine local-admin tables (047 shipped without them). Table-level
        // permissions render in `INFO FOR DB`, not `INFO FOR TABLE`.
        let (store, _authority, _service, _management) = fixture().await;
        let rows = store
            .admin_query("INFO FOR DB;", None)
            .await
            .expect("db info");
        let row = rows.first().expect("one info envelope");
        for table in [
            "browser_auth_policy",
            "local_admin",
            "local_admin_challenge",
            "local_admin_session",
            "local_admin_rate_bucket",
            "local_admin_client",
            "local_admin_client_key",
            "local_admin_operation",
            "local_admin_audit",
        ] {
            let definition = row["tables"][table]
                .as_str()
                .unwrap_or_else(|| panic!("{table} has no table definition: {row}"));
            assert!(
                definition.contains("PERMISSIONS"),
                "{table} must declare permissions: {definition}"
            );
            assert!(
                definition.contains("WHERE false"),
                "{table} must deny unprivileged access: {definition}"
            );
        }
    }
}

#[cfg(test)]
mod secret_hygiene_tests {
    //! Plan §5: "Generated sentinel secrets through errors/Debug/logs/audit/
    //! metadata/UI storage" → absent except the authorized one-time hand-off.
    //!
    //! This drives the real service over the real durable store, then searches
    //! every local-admin table for the raw artifact. The only durable form of a
    //! challenge code, a session cookie or a password is a verifier, a cookie
    //! verifier or a PHC string, so a raw artifact found in a row would be a
    //! defect by construction.

    use super::*;
    use crate::error::MemoryError;
    use crate::service::local_admin::contracts::{
        AdminLogin, ChallengeKind, LocalAdminError, OneTimeChallenge,
    };

    use super::sql_fault_tests::{attempt, command_request, fixture};

    const PASSWORD: &str = "Sentinel-Passphrase-A1B2C3";

    /// Every local-admin table plus the browser-policy singleton.
    const TABLES: &[&str] = &[
        "browser_auth_policy",
        "local_admin",
        "local_admin_challenge",
        "local_admin_session",
        "local_admin_rate_bucket",
        "local_admin_client",
        "local_admin_client_key",
        "local_admin_operation",
        "local_admin_audit",
    ];

    /// Assert `needle` appears in no row of any local-admin table.
    async fn assert_absent_from_every_table(
        store: &SurrealRegistryStore,
        needle: &str,
        what: &str,
    ) {
        for table in TABLES {
            let rows = store
                .admin_query(&format!("SELECT * FROM {table};"), None)
                .await
                .expect("read table");
            let rendered = serde_json::to_string(&rows).expect("serialize rows");
            assert!(
                !rendered.contains(needle),
                "{what} must not be durably stored in {table}: {rendered}"
            );
        }
    }

    #[tokio::test]
    async fn sentinel_secrets_never_reach_debug_display_or_a_durable_row() {
        let (store, _authority, service, management) = fixture().await;

        // ── The one-time activation code ──────────────────────────────
        let challenge: OneTimeChallenge = management
            .create_admin("ops.one", &command_request())
            .await
            .expect("create admin");
        // Non-vacuous in the authorized direction: the caller does get the code.
        assert_eq!(challenge.code.len(), 64, "32-byte hex code");
        let rendered = format!("{challenge:?}");
        assert!(
            !rendered.contains(&challenge.code),
            "Debug must redact the activation code: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "the code field renders as the redaction marker: {rendered}"
        );
        assert_absent_from_every_table(&store, &challenge.code, "the activation code").await;

        // ── The password and the session cookie ───────────────────────
        service
            .finish_challenge(
                &attempt(),
                &challenge.code,
                ChallengeKind::Activate,
                PASSWORD.to_owned(),
            )
            .await
            .expect("activate");
        let login: AdminLogin = service
            .login(&attempt(), "ops.one", PASSWORD.to_owned())
            .await
            .expect("login");

        let rendered = format!("{login:?}");
        // The verifier hex must not render anywhere: `cookie_value` and the
        // fence's `session_id` (which IS the verifier) are both redacted.
        assert!(
            !rendered.contains(&login.cookie_value),
            "Debug must redact the session cookie: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "the redaction marker renders: {rendered}"
        );
        assert!(
            !rendered.contains(PASSWORD),
            "Debug must not carry the password: {rendered}"
        );

        // Durable rows: the *cookie as a header value* (name and verifier
        // together) must appear nowhere. The bare verifier hex is deliberately
        // stored — it is `local_admin_session`'s lookup key (`resolve_session`
        // selects `WHERE cookie_verifier = $hex`) — so scanning for the hex
        // alone would assert against the design; the composed cookie string is
        // what must never be persisted.
        let composed_cookie = format!(
            "{}={}",
            crate::control::local_admin::csrf::session_cookie_name(""),
            login.cookie_value
        );
        assert_absent_from_every_table(&store, &composed_cookie, "the session cookie").await;
        assert_absent_from_every_table(&store, PASSWORD, "the administrator password").await;
        // A consumed code must not linger either.
        assert_absent_from_every_table(&store, &challenge.code, "the consumed activation code")
            .await;

        // Non-vacuous in the other direction: the durable rows exist and hold a
        // derivation rather than the raw value.
        let admins = store
            .admin_query("SELECT password_phc FROM local_admin;", None)
            .await
            .expect("read the administrator");
        let phc = admins
            .first()
            .and_then(|row| row["password_phc"].as_str())
            .expect("stored PHC");
        assert!(
            phc.starts_with("$argon2id$"),
            "the stored form is a PHC hash: {phc}"
        );
        assert!(!phc.contains(PASSWORD), "a PHC never embeds the password");

        // ── Errors ────────────────────────────────────────────────────
        let infrastructure = LocalAdminError::Infrastructure(MemoryError::Storage(
            "SENTINEL-DATABASE-TEXT".to_owned(),
        ));
        let rendered = format!("{infrastructure:?}");
        assert_eq!(
            rendered, "Infrastructure(<redacted>)",
            "the wrapped infrastructure error must not be printed"
        );
        assert_eq!(
            infrastructure.to_string(),
            "internal error",
            "Display stays constant and safe"
        );
    }
}
