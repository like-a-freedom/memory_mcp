-- Migration 026: Make Cyrillic (Russian) stemming active on the FTS indexes.
--
-- Migration 025 defined a `memory_fts_ru` analyzer but never bound any index
-- to it, so Russian-language stemming was a no-op: the existing
-- `fact_content_search`, `community_summary_search`, and `fact_index_keys_search`
-- indexes all still used the `memory_fts` analyzer (English stemming only).
--
-- Rather than fragmenting the corpus across two analyzers, we extend the
-- single `memory_fts` analyzer with BOTH snowball filters. SurrealDB applies
-- filter chains in order; the `class` tokenizer is already script-agnostic
-- (works on Cyrillic and Latin alike), and the `ascii` filter is a no-op for
-- Cyrillic codepoints. Each stemmer leaves the other script's tokens
-- effectively untouched, so English text continues to stem as before and
-- Russian text now gets proper stemming.
--
-- `OVERWRITE` is required because `memory_fts` already exists (defined in
-- `__Initial.surql`, last overwritten by 006). Existing FULLTEXT indexes pick
-- up the new analyzer automatically after re-definition; no index rebuild is
-- required for query correctness (SurrealDB re-tokenizes at query time).
DEFINE ANALYZER OVERWRITE memory_fts
    TOKENIZERS class
    FILTERS lowercase, ascii, snowball(english), snowball(russian);
