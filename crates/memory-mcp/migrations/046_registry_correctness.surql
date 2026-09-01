-- 046_registry_correctness.surql
-- Durable identity and version invariants for the control namespace.
-- This migration is append-only; historical migration files are not edited.

-- A tenant has one owning account, and a plan version has one durable row.
DEFINE INDEX IF NOT EXISTS idx_account_tenant
    ON account FIELDS tenant_id UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_tenant_namespace_binding
    ON tenant FIELDS namespace_binding.namespace, namespace_binding.database UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_plan_version
    ON plan FIELDS version UNIQUE;

-- Make every persisted plan limit explicit and numeric. The loader still
-- decodes these fields strictly so malformed legacy rows fail closed.
DEFINE FIELD IF NOT EXISTS limits.max_ingested_bytes ON plan TYPE number
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS limits.max_episode_count ON plan TYPE number
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS limits.ingest_per_minute ON plan TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS limits.max_open_app_sessions ON plan TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS limits.max_active_api_keys ON plan TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS limits.per_tenant_request_concurrency ON plan TYPE int
    ASSERT $value >= 0;
DEFINE FIELD IF NOT EXISTS limits.extraction_concurrency ON plan TYPE int
    ASSERT $value >= 0;

