-- Breaking search redesign migration.
--
-- This is the single runtime upgrade step for existing databases.
-- Fresh installs already receive the target schema from `__Initial.surql`.

REMOVE INDEX IF EXISTS episode_embedding_hnsw ON TABLE episode;
REMOVE INDEX IF EXISTS entity_embedding_hnsw ON TABLE entity;
REMOVE INDEX IF EXISTS fact_embedding_hnsw ON TABLE fact;

REMOVE FIELD IF EXISTS embedding ON TABLE episode;
REMOVE FIELD IF EXISTS embedding ON TABLE entity;
REMOVE FIELD IF EXISTS embedding ON TABLE fact;
REMOVE INDEX IF EXISTS edge_from ON TABLE edge;
REMOVE INDEX IF EXISTS edge_to ON TABLE edge;

DEFINE FIELD OVERWRITE t_ref ON episode TYPE datetime;
DEFINE FIELD OVERWRITE t_ingested ON episode TYPE datetime;

DEFINE FIELD OVERWRITE t_valid ON fact TYPE datetime;
DEFINE FIELD OVERWRITE t_ingested ON fact TYPE datetime;
DEFINE FIELD OVERWRITE t_invalid ON fact TYPE option<datetime>;
DEFINE FIELD OVERWRITE t_invalid_ingested ON fact TYPE option<datetime>;
DEFINE FIELD OVERWRITE entity_links ON fact TYPE array;

DEFINE FIELD OVERWRITE in ON edge TYPE record;
DEFINE FIELD OVERWRITE out ON edge TYPE record;
DEFINE FIELD OVERWRITE t_valid ON edge TYPE datetime;
DEFINE FIELD OVERWRITE t_ingested ON edge TYPE datetime;
DEFINE FIELD OVERWRITE t_invalid ON edge TYPE option<datetime>;
DEFINE FIELD OVERWRITE t_invalid_ingested ON edge TYPE option<datetime>;

DEFINE FIELD OVERWRITE updated_at ON community TYPE datetime;
DEFINE FIELD OVERWRITE ts ON event_log TYPE datetime;
DEFINE FIELD OVERWRITE due_date ON task TYPE option<datetime>;
DEFINE FIELD OVERWRITE executed_at ON script_migration TYPE datetime;
DEFINE FIELD OVERWRITE checksum ON script_migration TYPE option<string>;

DEFINE ANALYZER OVERWRITE memory_fts TOKENIZERS class FILTERS lowercase, ascii, snowball(english);

DEFINE INDEX OVERWRITE fact_content_search ON TABLE fact COLUMNS content FULLTEXT ANALYZER memory_fts;
DEFINE INDEX OVERWRITE community_summary_search ON TABLE community COLUMNS summary FULLTEXT ANALYZER memory_fts;
DEFINE INDEX OVERWRITE edge_relation ON TABLE edge COLUMNS relation;
DEFINE INDEX OVERWRITE edge_in ON TABLE edge COLUMNS in;
DEFINE INDEX OVERWRITE edge_out ON TABLE edge COLUMNS out;
DEFINE INDEX OVERWRITE community_members ON TABLE community COLUMNS member_entities;

UPDATE edge SET in = type::record(from_id) WHERE in IS NONE AND from_id IS NOT NONE;
UPDATE edge SET out = type::record(to_id) WHERE out IS NONE AND to_id IS NOT NONE;

REMOVE FIELD IF EXISTS from_id ON TABLE edge;
REMOVE FIELD IF EXISTS to_id ON TABLE edge;