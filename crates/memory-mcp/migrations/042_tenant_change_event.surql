-- Tenant-local change event outbox (spec §11, Phase 9).
-- The sequence table holds one row per tenant namespace.
-- The event table stores the durable change log that
-- subscriptions/replay reads from.

DEFINE TABLE IF NOT EXISTS tenant_change_sequence SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS value ON tenant_change_sequence TYPE int DEFAULT 0;

DEFINE TABLE IF NOT EXISTS tenant_change_event SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS event_seq ON tenant_change_event TYPE int;
DEFINE FIELD IF NOT EXISTS resource_id ON tenant_change_event TYPE string;
DEFINE FIELD IF NOT EXISTS rev ON tenant_change_event TYPE int;
DEFINE FIELD IF NOT EXISTS change_kind ON tenant_change_event TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON tenant_change_event TYPE datetime;
DEFINE INDEX IF NOT EXISTS idx_event_seq ON tenant_change_event FIELDS event_seq;
DEFINE INDEX IF NOT EXISTS idx_event_seq_unique ON tenant_change_event FIELDS event_seq UNIQUE;
