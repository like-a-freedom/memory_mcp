-- Migration 031: Entity-extraction projection (append-only).
--
-- Persists one row per successful entity-extraction run. Each row records the
-- resolved entity ids produced by that run plus the durable extractor
-- fingerprint (selector, backend, repository, revision, artifact identity,
-- labels, threshold, revision/validation status, runtime version, effective
-- device) that produced them, so historical outputs stay attributable even
-- after the extractor is upgraded or reconfigured.
--
-- Record ids are `entity_extraction_projection:<episode-key>:<projection-id>`
-- where `<episode-key>` is the episode id body and `<projection-id>` derives
-- deterministically from the episode id, scope, and ingestion timestamp.
--
-- Append-only: rows are never updated or deleted. Re-running extraction on an
-- episode appends a new row; earlier rows form the durable attribution trail.
-- Applies after 030; never edits earlier migrations.

DEFINE TABLE entity_extraction_projection SCHEMAFULL;
DEFINE FIELD episode_id ON entity_extraction_projection TYPE string;
DEFINE FIELD scope ON entity_extraction_projection TYPE string;
DEFINE FIELD t_ingested ON entity_extraction_projection TYPE datetime;
DEFINE FIELD t_created ON entity_extraction_projection TYPE datetime;
DEFINE FIELD fingerprint ON entity_extraction_projection TYPE object FLEXIBLE;
DEFINE FIELD entity_ids ON entity_extraction_projection TYPE array;

DEFINE INDEX entity_extraction_projection_episode_idx
    ON entity_extraction_projection COLUMNS episode_id;
DEFINE INDEX entity_extraction_projection_ingested_idx
    ON entity_extraction_projection COLUMNS t_ingested;
