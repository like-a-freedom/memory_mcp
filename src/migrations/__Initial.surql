DEFINE TABLE episode SCHEMAFULL;
DEFINE FIELD episode_id ON episode TYPE string;
DEFINE FIELD source_type ON episode TYPE string;
DEFINE FIELD source_id ON episode TYPE string;
DEFINE FIELD content ON episode TYPE string;
DEFINE FIELD t_ref ON episode TYPE string;
DEFINE FIELD t_ingested ON episode TYPE string;
DEFINE FIELD scope ON episode TYPE string;
DEFINE FIELD visibility_scope ON episode TYPE string;
DEFINE FIELD policy_tags ON episode TYPE array;

DEFINE TABLE entity SCHEMAFULL;
DEFINE FIELD entity_id ON entity TYPE string;
DEFINE FIELD entity_type ON entity TYPE string;
DEFINE FIELD canonical_name ON entity TYPE string;
DEFINE FIELD aliases ON entity TYPE array;

DEFINE TABLE fact SCHEMAFULL;
DEFINE FIELD fact_id ON fact TYPE string;
DEFINE FIELD fact_type ON fact TYPE string;
DEFINE FIELD content ON fact TYPE string;
DEFINE FIELD quote ON fact TYPE string;
DEFINE FIELD source_episode ON fact TYPE string;
DEFINE FIELD t_valid ON fact TYPE string;
DEFINE FIELD t_ingested ON fact TYPE string;
DEFINE FIELD t_invalid ON fact TYPE option<string>;
DEFINE FIELD t_invalid_ingested ON fact TYPE option<string>;
DEFINE FIELD confidence ON fact TYPE number;
DEFINE FIELD entity_links ON fact TYPE array;
DEFINE FIELD scope ON fact TYPE string;
DEFINE FIELD policy_tags ON fact TYPE array;
DEFINE FIELD provenance ON fact TYPE object;

DEFINE TABLE edge SCHEMAFULL;
DEFINE FIELD edge_id ON edge TYPE string;
DEFINE FIELD from_id ON edge TYPE string;
DEFINE FIELD relation ON edge TYPE string;
DEFINE FIELD to_id ON edge TYPE string;
DEFINE FIELD strength ON edge TYPE number;
DEFINE FIELD confidence ON edge TYPE number;
DEFINE FIELD provenance ON edge TYPE object;
DEFINE FIELD t_valid ON edge TYPE string;
DEFINE FIELD t_ingested ON edge TYPE string;
DEFINE FIELD t_invalid ON edge TYPE option<string>;
DEFINE FIELD t_invalid_ingested ON edge TYPE option<string>;

DEFINE TABLE community SCHEMAFULL;
DEFINE FIELD community_id ON community TYPE string;
DEFINE FIELD member_entities ON community TYPE array;
DEFINE FIELD summary ON community TYPE string;
DEFINE FIELD updated_at ON community TYPE string;

DEFINE TABLE event_log SCHEMAFULL;
DEFINE FIELD ts ON event_log TYPE string;
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
DEFINE FIELD due_date ON task TYPE option<string>;

DEFINE TABLE script_migration SCHEMAFULL;
DEFINE FIELD script_name ON script_migration TYPE string;
DEFINE FIELD executed_at ON script_migration TYPE string DEFAULT <string>time::now();
DEFINE FIELD checksum ON script_migration TYPE option<string>;

DEFINE ANALYZER simple TOKENIZERS blank FILTERS lowercase;

DEFINE INDEX episode_source_id ON TABLE episode COLUMNS source_type, source_id;
DEFINE INDEX entity_canonical_name ON TABLE entity COLUMNS canonical_name;
DEFINE INDEX fact_content_search ON TABLE fact COLUMNS content FULLTEXT ANALYZER simple;
DEFINE INDEX edge_relation ON TABLE edge COLUMNS relation;
DEFINE INDEX community_members ON TABLE community COLUMNS member_entities;
