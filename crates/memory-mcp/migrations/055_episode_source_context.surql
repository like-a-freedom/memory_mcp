DEFINE FIELD source_group ON episode TYPE option<string>;
DEFINE FIELD source_position ON episode TYPE option<int>;
DEFINE INDEX episode_source_context ON TABLE episode COLUMNS source_group, source_position;
