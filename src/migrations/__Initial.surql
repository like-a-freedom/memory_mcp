DEFINE TABLE episode SCHEMAFULL;
DEFINE FIELD episode_id ON episode TYPE string;
DEFINE FIELD source_type ON episode TYPE string;
DEFINE FIELD source_id ON episode TYPE string;
DEFINE FIELD content ON episode TYPE string;
DEFINE FIELD t_ref ON episode TYPE datetime;
DEFINE FIELD t_ingested ON episode TYPE datetime;
DEFINE FIELD scope ON episode TYPE string;
DEFINE FIELD visibility_scope ON episode TYPE string;
DEFINE FIELD policy_tags ON episode TYPE array;
DEFINE FIELD embedding ON episode TYPE option<array<number>>;

DEFINE TABLE entity SCHEMAFULL;
DEFINE FIELD entity_id ON entity TYPE string;
DEFINE FIELD entity_type ON entity TYPE string;
DEFINE FIELD canonical_name ON entity TYPE string;
DEFINE FIELD canonical_name_normalized ON entity TYPE string;
DEFINE FIELD aliases ON entity TYPE array;
DEFINE FIELD embedding ON entity TYPE option<array<number>>;

DEFINE TABLE fact SCHEMAFULL;
DEFINE FIELD fact_id ON fact TYPE string;
DEFINE FIELD fact_type ON fact TYPE string;
DEFINE FIELD content ON fact TYPE string;
DEFINE FIELD quote ON fact TYPE string;
DEFINE FIELD source_episode ON fact TYPE string;
DEFINE FIELD t_valid ON fact TYPE datetime;
DEFINE FIELD t_ingested ON fact TYPE datetime;
DEFINE FIELD t_invalid ON fact TYPE option<datetime>;
DEFINE FIELD t_invalid_ingested ON fact TYPE option<datetime>;
DEFINE FIELD confidence ON fact TYPE number;
DEFINE FIELD entity_links ON fact TYPE array;
DEFINE FIELD scope ON fact TYPE string;
DEFINE FIELD policy_tags ON fact TYPE array;
DEFINE FIELD provenance ON fact TYPE object FLEXIBLE;
DEFINE FIELD embedding ON fact TYPE option<array<number>>;

DEFINE TABLE edge TYPE RELATION;
DEFINE FIELD edge_id ON edge TYPE string;
DEFINE FIELD from_id ON edge TYPE string;
DEFINE FIELD relation ON edge TYPE string;
DEFINE FIELD to_id ON edge TYPE string;
DEFINE FIELD strength ON edge TYPE number;
DEFINE FIELD confidence ON edge TYPE number;
DEFINE FIELD provenance ON edge TYPE object FLEXIBLE;
DEFINE FIELD t_valid ON edge TYPE datetime;
DEFINE FIELD t_ingested ON edge TYPE datetime;
DEFINE FIELD t_invalid ON edge TYPE option<datetime>;
DEFINE FIELD t_invalid_ingested ON edge TYPE option<datetime>;

DEFINE TABLE community SCHEMAFULL;
DEFINE FIELD community_id ON community TYPE string;
DEFINE FIELD member_entities ON community TYPE array;
DEFINE FIELD summary ON community TYPE string;
DEFINE FIELD updated_at ON community TYPE datetime;

DEFINE TABLE event_log SCHEMAFULL;
DEFINE FIELD ts ON event_log TYPE datetime;
DEFINE FIELD op ON event_log TYPE string;
DEFINE FIELD args ON event_log TYPE object;
DEFINE FIELD result ON event_log TYPE object;
DEFINE FIELD access ON event_log TYPE object;
DEFINE FIELD transport ON event_log TYPE string;
DEFINE FIELD content_type ON event_log TYPE string;
DEFINE FIELD session_vars ON event_log TYPE object;

DEFINE TABLE task SCHEMAFULL;
DEFINE FIELD status ON task TYPE string;
DEFINE FIELD title ON task TYPE string;
DEFINE FIELD due_date ON task TYPE option<datetime>;

DEFINE TABLE script_migration SCHEMAFULL;
DEFINE FIELD script_name ON script_migration TYPE string;
DEFINE FIELD executed_at ON script_migration TYPE datetime DEFAULT time::now();
DEFINE FIELD checksum ON script_migration TYPE option<string>;

DEFINE ANALYZER simple TOKENIZERS blank FILTERS lowercase;

DEFINE INDEX episode_source_id ON TABLE episode COLUMNS source_type, source_id;
DEFINE INDEX entity_canonical_name ON TABLE entity COLUMNS canonical_name;
DEFINE INDEX entity_canonical_name_normalized ON TABLE entity COLUMNS canonical_name_normalized;
DEFINE INDEX entity_aliases ON TABLE entity COLUMNS aliases;
DEFINE INDEX fact_content_search ON TABLE fact COLUMNS content FULLTEXT ANALYZER simple;
DEFINE INDEX community_summary_search ON TABLE community COLUMNS summary FULLTEXT ANALYZER simple;
DEFINE INDEX episode_embedding_hnsw ON episode FIELDS embedding HNSW DIMENSION {{EMBEDDING_DIMENSION}} DIST COSINE TYPE F32 EFC 150 M 8;
DEFINE INDEX entity_embedding_hnsw ON entity FIELDS embedding HNSW DIMENSION {{EMBEDDING_DIMENSION}} DIST COSINE TYPE F32 EFC 150 M 8;
DEFINE INDEX fact_embedding_hnsw ON fact FIELDS embedding HNSW DIMENSION {{EMBEDDING_DIMENSION}} DIST COSINE TYPE F32 EFC 150 M 8;
DEFINE INDEX edge_relation ON TABLE edge COLUMNS relation;
DEFINE INDEX edge_from_id ON TABLE edge COLUMNS from_id;
DEFINE INDEX edge_to_id ON TABLE edge COLUMNS to_id;
DEFINE INDEX community_members ON TABLE community COLUMNS member_entities;
