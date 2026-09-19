-- 047_local_admin_auth.surql
-- Local administrator authentication and client provisioning schema.
-- Approved migration for local admin auth tables.
-- Never edit 001/045/046.

-- ─── browser_auth_policy ──────────────────────────────────
DEFINE TABLE browser_auth_policy SCHEMAFULL;
DEFINE FIELD mode ON browser_auth_policy TYPE string;
DEFINE FIELD epoch ON browser_auth_policy TYPE int;
DEFINE FIELD version ON browser_auth_policy TYPE int;
DEFINE FIELD local_session_fingerprint ON browser_auth_policy TYPE option<string>;
DEFINE FIELD local_csrf_fingerprint ON browser_auth_policy TYPE option<string>;
DEFINE FIELD created_at ON browser_auth_policy TYPE datetime;
DEFINE FIELD updated_at ON browser_auth_policy TYPE datetime;
DEFINE INDEX idx_browser_auth_policy_singleton ON browser_auth_policy FIELDS mode UNIQUE;

-- ─── local_admin ──────────────────────────────────────────
DEFINE TABLE local_admin SCHEMAFULL;
DEFINE FIELD id ON local_admin TYPE string;
DEFINE FIELD username ON local_admin TYPE string;
DEFINE FIELD state ON local_admin TYPE string;
DEFINE FIELD password_phc ON local_admin TYPE option<string>;
DEFINE FIELD credential_generation ON local_admin TYPE int;
DEFINE FIELD version ON local_admin TYPE int;
DEFINE FIELD created_at ON local_admin TYPE datetime;
DEFINE FIELD updated_at ON local_admin TYPE datetime;
DEFINE INDEX idx_local_admin_id ON local_admin FIELDS id UNIQUE;
DEFINE INDEX idx_local_admin_username ON local_admin FIELDS username UNIQUE;

-- ─── local_admin_challenge ────────────────────────────────
DEFINE TABLE local_admin_challenge SCHEMAFULL;
DEFINE FIELD id ON local_admin_challenge TYPE string;
DEFINE FIELD admin_id ON local_admin_challenge TYPE string;
DEFINE FIELD kind ON local_admin_challenge TYPE string;
DEFINE FIELD verifier ON local_admin_challenge TYPE string;
DEFINE FIELD credential_generation ON local_admin_challenge TYPE int;
DEFINE FIELD mode_epoch ON local_admin_challenge TYPE int;
DEFINE FIELD expires_at ON local_admin_challenge TYPE datetime;
DEFINE FIELD consumed_at ON local_admin_challenge TYPE option<datetime>;
DEFINE FIELD revoked_at ON local_admin_challenge TYPE option<datetime>;
DEFINE FIELD created_at ON local_admin_challenge TYPE datetime;
DEFINE INDEX idx_local_admin_challenge_verifier ON local_admin_challenge FIELDS verifier UNIQUE;
DEFINE INDEX idx_local_admin_challenge_admin ON local_admin_challenge FIELDS admin_id;

-- ─── local_admin_session ──────────────────────────────────
DEFINE TABLE local_admin_session SCHEMAFULL;
DEFINE FIELD id ON local_admin_session TYPE string;
DEFINE FIELD cookie_verifier ON local_admin_session TYPE string;
DEFINE FIELD admin_id ON local_admin_session TYPE string;
DEFINE FIELD credential_generation ON local_admin_session TYPE int;
DEFINE FIELD mode_epoch ON local_admin_session TYPE int;
DEFINE FIELD auth_time ON local_admin_session TYPE datetime;
DEFINE FIELD idle_expiry ON local_admin_session TYPE datetime;
DEFINE FIELD absolute_expiry ON local_admin_session TYPE datetime;
DEFINE FIELD revoked_at ON local_admin_session TYPE option<datetime>;
DEFINE INDEX idx_local_admin_session_cookie ON local_admin_session FIELDS cookie_verifier UNIQUE;
DEFINE INDEX idx_local_admin_session_admin ON local_admin_session FIELDS admin_id;

-- ─── local_admin_rate_bucket ──────────────────────────────
DEFINE TABLE local_admin_rate_bucket SCHEMAFULL;
DEFINE FIELD bucket_id ON local_admin_rate_bucket TYPE string;
DEFINE FIELD window_start ON local_admin_rate_bucket TYPE datetime;
DEFINE FIELD count ON local_admin_rate_bucket TYPE int;
DEFINE FIELD expires_at ON local_admin_rate_bucket TYPE datetime;
DEFINE FIELD denied_count ON local_admin_rate_bucket TYPE int;
DEFINE INDEX idx_local_admin_rate_bucket_id ON local_admin_rate_bucket FIELDS bucket_id UNIQUE;

-- ─── local_admin_client ───────────────────────────────────
DEFINE TABLE local_admin_client SCHEMAFULL;
DEFINE FIELD account_id ON local_admin_client TYPE string;
DEFINE FIELD display_name ON local_admin_client TYPE string;
DEFINE FIELD creating_admin_id ON local_admin_client TYPE string;
DEFINE FIELD operation_id ON local_admin_client TYPE string;
DEFINE FIELD created_at ON local_admin_client TYPE datetime;
DEFINE FIELD version ON local_admin_client TYPE int;
DEFINE FIELD suspended_from_ready ON local_admin_client TYPE bool;
DEFINE INDEX idx_local_admin_client_account ON local_admin_client FIELDS account_id UNIQUE;
DEFINE INDEX idx_local_admin_client_admin ON local_admin_client FIELDS creating_admin_id;

-- ─── local_admin_operation ────────────────────────────────
DEFINE TABLE local_admin_operation SCHEMAFULL;
DEFINE FIELD id ON local_admin_operation TYPE string;
DEFINE FIELD admin_id ON local_admin_operation TYPE string;
DEFINE FIELD operation_kind ON local_admin_operation TYPE string;
DEFINE FIELD idempotency_id ON local_admin_operation TYPE string;
DEFINE FIELD request_fingerprint ON local_admin_operation TYPE string;
DEFINE FIELD result_resource_id ON local_admin_operation TYPE option<string>;
DEFINE FIELD committed_at ON local_admin_operation TYPE datetime;
DEFINE INDEX idx_local_admin_operation_unique ON local_admin_operation FIELDS admin_id, operation_kind, idempotency_id UNIQUE;

-- ─── local_admin_audit ────────────────────────────────────
DEFINE TABLE local_admin_audit SCHEMAFULL;
DEFINE FIELD id ON local_admin_audit TYPE string;
DEFINE FIELD event_time ON local_admin_audit TYPE datetime;
DEFINE FIELD actor_kind ON local_admin_audit TYPE string;
DEFINE FIELD actor_id ON local_admin_audit TYPE string;
DEFINE FIELD action ON local_admin_audit TYPE string;
DEFINE FIELD target_admin_id ON local_admin_audit TYPE option<string>;
DEFINE FIELD target_account_id ON local_admin_audit TYPE option<string>;
DEFINE FIELD target_tenant_id ON local_admin_audit TYPE option<string>;
DEFINE FIELD target_key_id ON local_admin_audit TYPE option<string>;
DEFINE FIELD outcome ON local_admin_audit TYPE string;
DEFINE FIELD reason ON local_admin_audit TYPE option<string>;
DEFINE FIELD request_id ON local_admin_audit TYPE string;
DEFINE FIELD grants_client_data_access ON local_admin_audit TYPE option<bool>;
DEFINE INDEX idx_local_admin_audit_request ON local_admin_audit FIELDS request_id;
