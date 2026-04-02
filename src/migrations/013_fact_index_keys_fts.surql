-- Add fulltext index on fact.index_keys for search across entity names and temporal markers.
-- This enables queries like "alice smith" to match facts linked to that entity.

DEFINE INDEX IF NOT EXISTS fact_index_keys_search ON TABLE fact COLUMNS index_keys FULLTEXT ANALYZER memory_fts;