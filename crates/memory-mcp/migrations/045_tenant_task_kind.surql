-- 045_tenant_task_kind.surql
-- Discriminate durable task payloads. Rows written before this
-- migration have no `kind`; readers treat an absent value as
-- `extract`, so in-flight extractions keep working across a deploy.

DEFINE FIELD IF NOT EXISTS kind ON tenant_task TYPE option<string>;