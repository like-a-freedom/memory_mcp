-- 045_deletion_and_usage_hardening.surql
-- Control-namespace deletion challenges/tombstones, usage rows,
-- and tenant audit hooks (Task 1, Task 11, Task 12).
--
-- Account, tenant, identity, namespace-binding, API-key, challenge,
-- session, and audit rows are durable and non-reusable. Deletion is
-- logical: lifecycle fields and terminal states are updated, never
-- physically removed. The deletion worker may physically remove only
-- expired tenant-local `app_session` rows.

-- ----- deletion_challenge -----
DEFINE TABLE IF NOT EXISTS deletion_challenge SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create WHERE false
        FOR update WHERE false
        FOR delete WHERE false;

DEFINE FIELD IF NOT EXISTS id ON deletion_challenge TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS verifier ON deletion_challenge TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS account_id ON deletion_challenge TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS session_id ON deletion_challenge TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS expires_at ON deletion_challenge TYPE datetime;
DEFINE FIELD IF NOT EXISTS consumed_at ON deletion_challenge TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS created_at ON deletion_challenge TYPE datetime
    DEFAULT time::now();

DEFINE INDEX IF NOT EXISTS idx_deletion_challenge_verifier
    ON deletion_challenge FIELDS verifier UNIQUE;

-- ----- usage -----
DEFINE TABLE IF NOT EXISTS usage SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS tenant_id ON usage TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS ingested_bytes ON usage TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS episode_count ON usage TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS open_app_sessions ON usage TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS active_api_keys ON usage TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS ingest_window_start ON usage TYPE datetime;
DEFINE FIELD IF NOT EXISTS ingest_current_minute ON usage TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS updated_at ON usage TYPE datetime
    DEFAULT time::now();

DEFINE INDEX IF NOT EXISTS idx_usage_tenant
    ON usage FIELDS tenant_id UNIQUE;

-- ----- account tombstone (terminal Deleting+Purged record) -----
DEFINE FIELD IF NOT EXISTS deletion_challenge_id ON account TYPE option<string>;
DEFINE FIELD IF NOT EXISTS deletion_started_at ON account TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS deletion_completed_at ON account TYPE option<datetime>;

-- ----- tenant tombstone (terminal Purged record) -----
DEFINE FIELD IF NOT EXISTS deletion_started_at ON tenant TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS deletion_completed_at ON tenant TYPE option<datetime>;

-- A retained tenant tombstone permanently reserves its namespace. This
-- prevents a later account from being routed to historical tenant data.
DEFINE INDEX IF NOT EXISTS idx_tenant_namespace_binding
    ON tenant FIELDS namespace_binding.namespace UNIQUE;

-- ----- control_plane_session extras for durable cookies -----
DEFINE TABLE IF NOT EXISTS control_plane_session SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS id ON control_plane_session TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS cookie_hash ON control_plane_session TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS account_id ON control_plane_session TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS auth_time ON control_plane_session TYPE datetime;
DEFINE FIELD IF NOT EXISTS idle_expiry ON control_plane_session TYPE datetime;
DEFINE FIELD IF NOT EXISTS absolute_expiry ON control_plane_session TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_control_plane_session_cookie
    ON control_plane_session FIELDS cookie_hash UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_control_plane_session_account
    ON control_plane_session FIELDS account_id;

-- ----- oidc_request (sealed flow material) -----
DEFINE TABLE IF NOT EXISTS oidc_request SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create WHERE false
        FOR update WHERE false
        FOR delete WHERE false;

DEFINE FIELD IF NOT EXISTS state_hash ON oidc_request TYPE string
    ASSERT $value != NONE AND $value != "";
-- The payload is already AEAD ciphertext; base64 keeps this schema portable
-- between local and remote SurrealDB transports.
DEFINE FIELD IF NOT EXISTS sealed_payload ON oidc_request TYPE string;
DEFINE FIELD IF NOT EXISTS aead_nonce ON oidc_request TYPE array
    ASSERT array::len($value) = 12;
DEFINE FIELD IF NOT EXISTS expires_at ON oidc_request TYPE datetime;
DEFINE FIELD IF NOT EXISTS created_at ON oidc_request TYPE datetime
    DEFAULT time::now();

DEFINE INDEX IF NOT EXISTS idx_oidc_request_state
    ON oidc_request FIELDS state_hash UNIQUE;

-- ----- audit_event (durable non-reusable log of destructive control actions) -----
DEFINE TABLE IF NOT EXISTS audit_event SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create WHERE false
        FOR update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS account_id ON audit_event TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS actor_kind ON audit_event TYPE string
    ASSERT $value IN ["account", "operator", "system"];
DEFINE FIELD IF NOT EXISTS actor_principal ON audit_event TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS action ON audit_event TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS occurred_at ON audit_event TYPE datetime
    DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS correlation_id ON audit_event TYPE string
    ASSERT $value != NONE AND $value != "";

DEFINE INDEX IF NOT EXISTS idx_audit_event_account
    ON audit_event FIELDS account_id, occurred_at;
DEFINE INDEX IF NOT EXISTS idx_audit_event_action_correlation
    ON audit_event FIELDS action, correlation_id UNIQUE;

-- ----- migration_ledger (durable catalog state for control namespace) -----
DEFINE TABLE IF NOT EXISTS migration_ledger SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS file_name ON migration_ledger TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS checksum ON migration_ledger TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS status ON migration_ledger TYPE string
    ASSERT $value IN ["applying", "completed", "failed"];
DEFINE FIELD IF NOT EXISTS started_at ON migration_ledger TYPE datetime
    DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS completed_at ON migration_ledger TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS error ON migration_ledger TYPE option<string>;
DEFINE FIELD IF NOT EXISTS owner ON migration_ledger TYPE option<string>;
DEFINE FIELD IF NOT EXISTS lease_expires_at ON migration_ledger TYPE option<datetime>;

DEFINE INDEX IF NOT EXISTS idx_migration_ledger_file
    ON migration_ledger FIELDS file_name UNIQUE;