-- Durable Tenant Task record schema (spec §10).
-- The tenant_task table holds extraction tasks with
-- optimistic versioning and a fenced lease. Rows are
-- ephemeral: the retention job deletes rows whose
-- retention_expiry has passed; the table never holds
-- facts or registry history.

DEFINE TABLE IF NOT EXISTS tenant_task SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS id ON tenant_task TYPE string;
DEFINE FIELD IF NOT EXISTS tenant_id ON tenant_task TYPE string;
DEFINE FIELD IF NOT EXISTS fingerprint ON tenant_task TYPE string;
DEFINE FIELD IF NOT EXISTS state ON tenant_task TYPE string;
DEFINE FIELD IF NOT EXISTS version ON tenant_task TYPE int;
DEFINE FIELD IF NOT EXISTS lease_owner ON tenant_task TYPE option<string>;
DEFINE FIELD IF NOT EXISTS lease_generation ON tenant_task TYPE option<int>;
DEFINE FIELD IF NOT EXISTS lease_expiry ON tenant_task TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS cancellation_intent ON tenant_task TYPE option<bool>;
DEFINE FIELD IF NOT EXISTS params ON tenant_task TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS progress ON tenant_task TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS result ON tenant_task TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS error ON tenant_task TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS created_at ON tenant_task TYPE datetime;
DEFINE FIELD IF NOT EXISTS updated_at ON tenant_task TYPE datetime;
DEFINE FIELD IF NOT EXISTS retention_expiry ON tenant_task TYPE datetime;
DEFINE INDEX IF NOT EXISTS idx_tenant_task_state ON tenant_task FIELDS state;
DEFINE INDEX IF NOT EXISTS idx_tenant_task_fingerprint ON tenant_task FIELDS fingerprint;
