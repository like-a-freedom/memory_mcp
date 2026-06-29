-- Migration 025: Add Cyrillic FTS analyzer for Russian-language content.
--
-- The default `memory_fts` analyzer uses `snowball(english)`. This adds a
-- Russian-language analyzer for namespaces that primarily contain Cyrillic content.
-- SurrealDB 3.x supports `snowball(russian)`.

DEFINE ANALYZER memory_fts_ru TOKENIZERS class FILTERS lowercase, snowball(russian);
