-- 001_registry.surql
-- Control-namespace schema for the registry store (ADR-0052,
-- plan §4.1 Step 4). Phase 4 ships the SQL; Task 5.x wires the
-- runtime loader to apply it on connect.
--
-- All DEFINE statements are idempotent (`IF NOT EXISTS`) so
-- repeated application is safe.

-- ----- account -----
DEFINE TABLE IF NOT EXISTS account SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS id ON account TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS status ON account TYPE string
    ASSERT $value IN ["active", "suspended", "deleting"];
DEFINE FIELD IF NOT EXISTS tenant_id ON account TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS created_at ON account TYPE datetime
    DEFAULT time::now();

-- ----- tenant -----
DEFINE TABLE IF NOT EXISTS tenant SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS id ON tenant TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS status ON tenant TYPE string
    ASSERT $value IN [
        "reserved", "namespace_creating", "migrating", "ready",
        "suspended", "failed", "deleting", "purged"
    ];
DEFINE FIELD IF NOT EXISTS plan_version ON tenant TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS schema_version ON tenant TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS retry_stage ON tenant TYPE option<string>;
DEFINE FIELD IF NOT EXISTS provisioning_lease ON tenant TYPE option<object>;
DEFINE FIELD IF NOT EXISTS version ON tenant TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS created_at ON tenant TYPE datetime
    DEFAULT time::now();

DEFINE FIELD IF NOT EXISTS namespace_binding ON tenant TYPE object;
DEFINE FIELD IF NOT EXISTS namespace_binding.namespace ON tenant TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS namespace_binding.database ON tenant TYPE string
    ASSERT $value != NONE AND $value != "";

-- ----- api_key -----
DEFINE TABLE IF NOT EXISTS api_key SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS id ON api_key TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS account_id ON api_key TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS name ON api_key TYPE string;
DEFINE FIELD IF NOT EXISTS verifier ON api_key TYPE object;
DEFINE FIELD IF NOT EXISTS verifier.0 ON api_key TYPE array;
DEFINE FIELD IF NOT EXISTS status ON api_key TYPE string
    ASSERT $value IN ["active", "revoked"];
DEFINE FIELD IF NOT EXISTS created_at ON api_key TYPE datetime
    DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS expires_at ON api_key TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS last_used_at ON api_key TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS version ON api_key TYPE int
    ASSERT $value >= 0;

-- ----- external_identity -----
DEFINE TABLE IF NOT EXISTS external_identity SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS id ON external_identity TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS issuer ON external_identity TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS subject_verifier ON external_identity TYPE object;
DEFINE FIELD IF NOT EXISTS subject_verifier.0 ON external_identity TYPE array;
DEFINE FIELD IF NOT EXISTS account_id ON external_identity TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS created_at ON external_identity TYPE datetime
    DEFAULT time::now();

DEFINE INDEX IF NOT EXISTS idx_external_identity_issuer_subject
    ON external_identity FIELDS issuer, subject_verifier UNIQUE;

-- ----- provisioning_event -----
DEFINE TABLE IF NOT EXISTS provisioning_event SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create WHERE false
        FOR update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS tenant_id ON provisioning_event TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS stage ON provisioning_event TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS created_at ON provisioning_event TYPE datetime
    DEFAULT time::now();

-- ----- plan -----
DEFINE TABLE IF NOT EXISTS plan SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS id ON plan TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS version ON plan TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS limits ON plan TYPE object;
