DEFINE FIELD index_keys ON fact TYPE array<string> DEFAULT [];
DEFINE FIELD access_count ON fact TYPE int DEFAULT 0;
DEFINE FIELD last_accessed ON fact TYPE option<datetime>;

UPDATE fact SET index_keys = [] WHERE index_keys = NONE;
UPDATE fact SET access_count = 0 WHERE access_count = NONE;

DEFINE INDEX fact_index_keys_search ON TABLE fact COLUMNS index_keys FULLTEXT ANALYZER memory_fts;
