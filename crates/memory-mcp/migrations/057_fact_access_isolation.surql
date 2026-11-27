-- Memory-owned mutable heat; knowledge facts retain their immutable legacy fields.
DEFINE TABLE IF NOT EXISTS fact_access SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS source_episode ON fact_access TYPE string;
DEFINE FIELD IF NOT EXISTS access_count ON fact_access TYPE int;
DEFINE FIELD IF NOT EXISTS last_accessed ON fact_access TYPE option<datetime>;
DEFINE INDEX IF NOT EXISTS fact_access_source_recent ON fact_access FIELDS source_episode, last_accessed;
-- Updated atomically with each bounded backfill batch by the migration runner.
DEFINE FIELD IF NOT EXISTS fact_access_cursor ON script_migration TYPE option<string>;
