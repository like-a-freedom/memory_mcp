-- Make the tenant_task fingerprint index unique to
-- prevent concurrent duplicate enqueues at the DB level.
-- The old non-unique index is dropped and replaced.

REMOVE INDEX IF EXISTS idx_tenant_task_fingerprint ON tenant_task;
DEFINE INDEX IF NOT EXISTS idx_tenant_task_fingerprint ON tenant_task FIELDS tenant_id, fingerprint UNIQUE;
