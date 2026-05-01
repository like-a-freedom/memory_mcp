-- Add richer retrieval diagnostics to assemble_context query analytics.

DEFINE FIELD OVERWRITE resolved_view_mode ON query_log TYPE option<string>;
DEFINE FIELD OVERWRITE query_flags ON query_log TYPE array<string>;
DEFINE FIELD OVERWRITE retrieval_tiers ON query_log TYPE option<object>;

DEFINE INDEX OVERWRITE query_log_scope_resolved_view_logged_at
    ON TABLE query_log COLUMNS scope, resolved_view_mode, logged_at;
