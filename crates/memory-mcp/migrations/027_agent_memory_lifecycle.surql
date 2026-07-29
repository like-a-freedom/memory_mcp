-- Migration 027: Agent-memory lifecycle event and durable projection job storage.
--
-- Stores bounded lifecycle events, durable projection jobs, and rejection
-- audits. Accepted content is stored once in the episode table; the event
-- references the episode without copying content. Ignored and duplicate events
-- create zero durable rows. Quarantine stores bounded isolated content with no
-- ordinary episode. Rejection stores hashes and reason codes only.
--
-- There is no per-recall receipt table. Exposure traces are ephemeral by
-- default; only a significant captured event copies a bounded trace link.
--
-- Note: the plan referenced this as migration 029, but migrations 027/028
-- (claim reconciliation) are not present in this worktree. The next available
-- append-only number is 027.

-- Lifecycle event: identity, provenance, hashes, disposition, trace link,
-- and episode reference.
DEFINE TABLE OVERWRITE memory_event SCHEMALESS;

DEFINE FIELD OVERWRITE event_id ON memory_event TYPE string;
DEFINE FIELD OVERWRITE adapter_id ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE adapter_version ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE host_event ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE session_id ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE native_event_id ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE event_kind ON memory_event TYPE string;
DEFINE FIELD OVERWRITE task_fingerprint ON memory_event TYPE string;
DEFINE FIELD OVERWRITE normalized_task ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON memory_event TYPE string;
DEFINE FIELD OVERWRITE project ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE policy_tags ON memory_event TYPE array DEFAULT [];
DEFINE FIELD OVERWRITE capture_signal ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE disposition ON memory_event TYPE string;
DEFINE FIELD OVERWRITE trust_class ON memory_event TYPE string;
DEFINE FIELD OVERWRITE source_kind ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE content_hash ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE content_byte_len ON memory_event TYPE option<int>;
DEFINE FIELD OVERWRITE artifact_uri_count ON memory_event TYPE option<int>;
DEFINE FIELD OVERWRITE reason_codes ON memory_event TYPE array DEFAULT [];
DEFINE FIELD OVERWRITE episode_id ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE trace_retrieval_fingerprint ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE trace_selected_fact_ids ON memory_event TYPE array DEFAULT [];
DEFINE FIELD OVERWRITE trace_selected_experience_ids ON memory_event TYPE array DEFAULT [];
DEFINE FIELD OVERWRITE trace_policy_fingerprint ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE origin_kind ON memory_event TYPE string;
DEFINE FIELD OVERWRITE created_at ON memory_event TYPE string;
DEFINE FIELD OVERWRITE expires_at ON memory_event TYPE option<string>;

DEFINE INDEX OVERWRITE memory_event_id ON TABLE memory_event COLUMNS event_id UNIQUE;
DEFINE INDEX OVERWRITE memory_event_session_kind ON TABLE memory_event COLUMNS session_id, event_kind;
DEFINE INDEX OVERWRITE memory_event_disposition ON TABLE memory_event COLUMNS disposition, created_at;
DEFINE INDEX OVERWRITE memory_event_scope_project ON TABLE memory_event COLUMNS scope, project;

-- Durable projection job: lease/retry/dead-letter state and episode reference.
DEFINE TABLE OVERWRITE event_projection_job SCHEMALESS;

DEFINE FIELD OVERWRITE job_id ON event_projection_job TYPE string;
DEFINE FIELD OVERWRITE event_id ON event_projection_job TYPE string;
DEFINE FIELD OVERWRITE episode_id ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON event_projection_job TYPE string;
DEFINE FIELD OVERWRITE project ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE status ON event_projection_job TYPE string;
DEFINE FIELD OVERWRITE attempts ON event_projection_job TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE max_attempts ON event_projection_job TYPE int DEFAULT 5;
DEFINE FIELD OVERWRITE leased_at ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE lease_expires_at ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE completed_at ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE last_error ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE dead_lettered_at ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE origin_kind ON event_projection_job TYPE string;
DEFINE FIELD OVERWRITE created_at ON event_projection_job TYPE string;
DEFINE FIELD OVERWRITE expires_at ON event_projection_job TYPE option<string>;

DEFINE INDEX OVERWRITE event_projection_job_id ON TABLE event_projection_job COLUMNS job_id UNIQUE;
DEFINE INDEX OVERWRITE event_projection_job_status ON TABLE event_projection_job COLUMNS status, created_at;
DEFINE INDEX OVERWRITE event_projection_job_lease ON TABLE event_projection_job COLUMNS status, lease_expires_at;
DEFINE INDEX OVERWRITE event_projection_job_event ON TABLE event_projection_job COLUMNS event_id;

-- Rejection audit: hashes and reason codes only for rejected content.
-- Raw secret-like content is never stored here.
DEFINE TABLE OVERWRITE memory_capture_audit SCHEMALESS;

DEFINE FIELD OVERWRITE audit_id ON memory_capture_audit TYPE string;
DEFINE FIELD OVERWRITE event_id ON memory_capture_audit TYPE string;
DEFINE FIELD OVERWRITE content_hash ON memory_capture_audit TYPE string;
DEFINE FIELD OVERWRITE content_byte_len ON memory_capture_audit TYPE int;
DEFINE FIELD OVERWRITE disposition ON memory_capture_audit TYPE string;
DEFINE FIELD OVERWRITE reason_codes ON memory_capture_audit TYPE array DEFAULT [];
DEFINE FIELD OVERWRITE scope ON memory_capture_audit TYPE string;
DEFINE FIELD OVERWRITE project ON memory_capture_audit TYPE option<string>;
DEFINE FIELD OVERWRITE created_at ON memory_capture_audit TYPE string;
DEFINE FIELD OVERWRITE expires_at ON memory_capture_audit TYPE option<string>;

DEFINE INDEX OVERWRITE memory_capture_audit_id ON TABLE memory_capture_audit COLUMNS audit_id UNIQUE;
DEFINE INDEX OVERWRITE memory_capture_audit_event ON TABLE memory_capture_audit COLUMNS event_id;
DEFINE INDEX OVERWRITE memory_capture_audit_disposition ON TABLE memory_capture_audit COLUMNS disposition, created_at;
