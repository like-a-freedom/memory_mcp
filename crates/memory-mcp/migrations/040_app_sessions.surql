-- App session schema. The `app_session` table holds
-- short-lived interactive state opened by an MCP client
-- during a session. Records are subject to idle and
-- absolute expiry; the cleanup job (Task 7.2 scheduler
-- pass) physically deletes rows whose expiry has
-- passed. This is the only TTL-governed ephemeral table
-- the cleanup job is allowed to touch; it never touches
-- facts or registry history.

DEFINE TABLE IF NOT EXISTS app_session SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS handle ON app_session TYPE string;
DEFINE FIELD IF NOT EXISTS tenant_id ON app_session TYPE string;
DEFINE FIELD IF NOT EXISTS app ON app_session TYPE string;
DEFINE FIELD IF NOT EXISTS version ON app_session TYPE int;
DEFINE FIELD IF NOT EXISTS payload ON app_session TYPE object FLEXIBLE;
DEFINE FIELD IF NOT EXISTS idle_expiry ON app_session TYPE datetime;
DEFINE FIELD IF NOT EXISTS absolute_expiry ON app_session TYPE datetime;
DEFINE INDEX IF NOT EXISTS idx_app_session_handle ON app_session FIELDS handle UNIQUE;
