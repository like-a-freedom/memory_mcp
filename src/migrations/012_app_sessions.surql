-- Migration: 012_app_sessions.surql
-- All Apps: Server-side session tracking

DEFINE TABLE app_session SCHEMAFULL;
DEFINE FIELD session_id ON app_session TYPE string;
DEFINE FIELD app_id ON app_session TYPE string;
DEFINE FIELD scope ON app_session TYPE string;
DEFINE FIELD access ON app_session TYPE object FLEXIBLE;
DEFINE FIELD target ON app_session TYPE object FLEXIBLE;
DEFINE FIELD state ON app_session TYPE string;
DEFINE FIELD created_at ON app_session TYPE datetime;
DEFINE FIELD last_active ON app_session TYPE datetime;
DEFINE FIELD ttl_seconds ON app_session TYPE int;

DEFINE INDEX app_session_session_id ON app_session COLUMNS session_id UNIQUE;
DEFINE INDEX app_session_scope ON app_session COLUMNS scope;
DEFINE INDEX app_session_app_id ON app_session COLUMNS app_id;
