-- Add server-owned knowledge commit revisions without fabricating history
-- for records written before this migration.

DEFINE TABLE IF NOT EXISTS knowledge_revision_clock SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS latest_revision ON knowledge_revision_clock TYPE int;
DEFINE FIELD IF NOT EXISTS last_recorded_at ON knowledge_revision_clock TYPE datetime;

DEFINE TABLE IF NOT EXISTS knowledge_revision SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS revision ON knowledge_revision TYPE int;
DEFINE FIELD IF NOT EXISTS recorded_at ON knowledge_revision TYPE datetime;
DEFINE FIELD IF NOT EXISTS operation_kind ON knowledge_revision TYPE string;
DEFINE FIELD IF NOT EXISTS affected_record_ids ON knowledge_revision TYPE array<string>;
DEFINE FIELD IF NOT EXISTS idempotency_key ON knowledge_revision TYPE string;
DEFINE INDEX IF NOT EXISTS knowledge_revision_number ON TABLE knowledge_revision FIELDS revision UNIQUE;
DEFINE INDEX IF NOT EXISTS knowledge_revision_idempotency ON TABLE knowledge_revision FIELDS idempotency_key UNIQUE;

DEFINE FIELD IF NOT EXISTS known_revision ON episode TYPE option<int>;
DEFINE FIELD IF NOT EXISTS known_revision ON fact TYPE option<int>;
DEFINE FIELD IF NOT EXISTS known_revision ON edge TYPE option<int>;
DEFINE FIELD IF NOT EXISTS known_revision ON triple TYPE option<int>;
DEFINE FIELD IF NOT EXISTS known_revision ON claim TYPE option<int>;
DEFINE FIELD IF NOT EXISTS known_revision ON claim_relation TYPE option<int>;

DEFINE FIELD IF NOT EXISTS closed_revision ON fact TYPE option<int>;
DEFINE FIELD IF NOT EXISTS closed_revision ON edge TYPE option<int>;
DEFINE FIELD IF NOT EXISTS closed_revision ON triple TYPE option<int>;
DEFINE FIELD IF NOT EXISTS closed_revision ON claim TYPE option<int>;
DEFINE FIELD IF NOT EXISTS closed_revision ON claim_relation TYPE option<int>;

LET $revision_clock = SELECT * FROM knowledge_revision_clock:active LIMIT 1;
IF array::len($revision_clock) = 0 {
    CREATE knowledge_revision_clock:active SET
        latest_revision = 0,
        last_recorded_at = time::now();
};
