-- Add fulltext index on episode.content for retrieval fallback.

DEFINE INDEX IF NOT EXISTS episode_content_search ON TABLE episode COLUMNS content FULLTEXT ANALYZER memory_fts;