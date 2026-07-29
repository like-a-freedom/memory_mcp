-- Add query analytics logging for assemble_context requests.

DEFINE TABLE OVERWRITE query_log SCHEMALESS;

DEFINE FIELD OVERWRITE query_log_id ON query_log TYPE string;
DEFINE FIELD OVERWRITE logged_at ON query_log TYPE string;
DEFINE FIELD OVERWRITE scope ON query_log TYPE string;
DEFINE FIELD OVERWRITE query ON query_log TYPE string;
DEFINE FIELD OVERWRITE project ON query_log TYPE option<string>;
DEFINE FIELD OVERWRITE view_mode ON query_log TYPE option<string>;
DEFINE FIELD OVERWRITE result_count ON query_log TYPE int;
DEFINE FIELD OVERWRITE latency_ms ON query_log TYPE float;
DEFINE FIELD OVERWRITE retrieval_tier ON query_log TYPE option<string>;
DEFINE FIELD OVERWRITE cache_hit ON query_log TYPE bool DEFAULT false;

DEFINE INDEX OVERWRITE query_log_scope_logged_at ON TABLE query_log COLUMNS scope, logged_at;
DEFINE INDEX OVERWRITE query_log_logged_at ON TABLE query_log COLUMNS logged_at;