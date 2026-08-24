-- Optional explicit source lineage for episodes ingested by the filesystem
-- watcher. Present only for internal filesystem ingestion; the public ingest
-- path never sets it. Claim projection prefers this stable lineage over the
-- versioned episode `source_id`.
DEFINE FIELD source_lineage ON episode TYPE option<string>;

-- Durable inbox revision store: immutable per-revision state owned by the
-- filesystem-ingestion pipeline inside `serve`. Migration history and models
-- compile with or without the `fs-watch` feature; only `notify` and runtime
-- startup are feature-gated.
DEFINE TABLE inbox_revision SCHEMAFULL;
DEFINE FIELD revision_id ON inbox_revision TYPE string;
DEFINE FIELD lineage ON inbox_revision TYPE string;
DEFINE FIELD relative_path ON inbox_revision TYPE string;
DEFINE FIELD content_sha256 ON inbox_revision TYPE string;
DEFINE FIELD source_type ON inbox_revision TYPE string;
DEFINE FIELD t_ref ON inbox_revision TYPE datetime;
DEFINE FIELD prepared_content ON inbox_revision TYPE option<string>;
DEFINE FIELD state ON inbox_revision TYPE string ASSERT $value IN ['discovered', 'processing', 'processed', 'failed'];
DEFINE FIELD processing_stage ON inbox_revision TYPE string ASSERT $value IN ['prepared', 'ingesting', 'extracting', 'complete'];
DEFINE FIELD expected_episode_id ON inbox_revision TYPE string;
DEFINE FIELD episode_id ON inbox_revision TYPE option<string>;
DEFINE FIELD attempt_count ON inbox_revision TYPE int;
DEFINE FIELD failure_count ON inbox_revision TYPE int;
DEFINE FIELD retry_generation ON inbox_revision TYPE option<string>;
DEFINE FIELD lease_owner ON inbox_revision TYPE option<string>;
DEFINE FIELD lease_expires_at ON inbox_revision TYPE option<datetime>;
DEFINE FIELD failure_class ON inbox_revision TYPE option<string>;
DEFINE FIELD last_error ON inbox_revision TYPE option<string>;
DEFINE FIELD discovered_at ON inbox_revision TYPE datetime;
DEFINE FIELD updated_at ON inbox_revision TYPE datetime;
DEFINE FIELD processed_at ON inbox_revision TYPE option<datetime>;
DEFINE INDEX inbox_revision_state_lease ON inbox_revision FIELDS state, lease_expires_at;
DEFINE INDEX inbox_revision_lineage_hash_unique ON inbox_revision FIELDS lineage, content_sha256 UNIQUE;
DEFINE INDEX inbox_revision_relative_path ON inbox_revision FIELDS relative_path;
