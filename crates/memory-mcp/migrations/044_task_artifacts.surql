-- 044_task_artifacts.surql
-- Tenant-namespace durable extraction artifact records (Task 7).
-- The artifact table is keyed by Task ID and stores bounded
-- extraction provenance. Tenant isolation is enforced by
-- `tenant_id` predicate; ordinary tenant credentials cannot
-- read foreign namespaces.

DEFINE TABLE IF NOT EXISTS task_artifact SCHEMAFULL
    PERMISSIONS
        FOR select WHERE tenant_id = $auth.tenant_id
        FOR create, update, delete WHERE false;

DEFINE FIELD IF NOT EXISTS task_id ON task_artifact TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS tenant_id ON task_artifact TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS fingerprint ON task_artifact TYPE string
    ASSERT $value != NONE AND $value != "";
DEFINE FIELD IF NOT EXISTS episode_id ON task_artifact TYPE option<string>;
DEFINE FIELD IF NOT EXISTS fact_ids ON task_artifact TYPE array
    ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS state ON task_artifact TYPE string
    ASSERT $value IN ["staged", "committed"];
DEFINE FIELD IF NOT EXISTS created_at ON task_artifact TYPE datetime
    DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS completed_at ON task_artifact TYPE option<datetime>;

DEFINE INDEX IF NOT EXISTS idx_task_artifact_task
    ON task_artifact FIELDS task_id UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_task_artifact_fingerprint
    ON task_artifact FIELDS tenant_id, fingerprint;