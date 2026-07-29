-- Migration 022: Add structured provenance sub-fields to fact and edge tables.
--
-- The `provenance` field already exists as TYPE object FLEXIBLE on both
-- `fact` and `edge`. This migration adds explicit sub-field definitions
-- so SurrealDB can validate and index them, and sets default values for
-- existing records.

-- Fact provenance sub-fields
DEFINE FIELD provenance.source_episode_id ON fact TYPE option<string>;
DEFINE FIELD provenance.source_url ON fact TYPE option<string>;
DEFINE FIELD provenance.ingestion_method ON fact TYPE string DEFAULT "manual";
DEFINE FIELD provenance.created_by ON fact TYPE option<string>;
DEFINE FIELD provenance.source_confidence ON fact TYPE option<float>;
DEFINE FIELD provenance.confidence_basis ON fact TYPE option<string>;
DEFINE FIELD provenance.extraction_strategy ON fact TYPE option<string>;
DEFINE FIELD provenance.source_type ON fact TYPE option<string>;
DEFINE FIELD provenance.source_id ON fact TYPE option<string>;

-- Edge provenance sub-fields
DEFINE FIELD provenance.source_episode_id ON edge TYPE option<string>;
DEFINE FIELD provenance.source_url ON edge TYPE option<string>;
DEFINE FIELD provenance.ingestion_method ON edge TYPE string DEFAULT "manual";
DEFINE FIELD provenance.created_by ON edge TYPE option<string>;
DEFINE FIELD provenance.source_confidence ON edge TYPE option<float>;
DEFINE FIELD provenance.confidence_basis ON edge TYPE option<string>;
DEFINE FIELD provenance.extraction_strategy ON edge TYPE option<string>;
DEFINE FIELD provenance.source_type ON edge TYPE option<string>;
DEFINE FIELD provenance.source_id ON edge TYPE option<string>;

-- Backfill: set ingestion_method for existing records that have a source_episode
-- but no ingestion_method (they were created via extraction).
UPDATE fact SET provenance.ingestion_method = "extraction"
    WHERE provenance.ingestion_method IS NONE
    AND provenance.source_episode IS NOT NONE;

UPDATE fact SET provenance.ingestion_method = "manual"
    WHERE provenance.ingestion_method IS NONE;

UPDATE edge SET provenance.ingestion_method = "extraction"
    WHERE provenance.ingestion_method IS NONE
    AND provenance.source_episode IS NOT NONE;

UPDATE edge SET provenance.ingestion_method = "manual"
    WHERE provenance.ingestion_method IS NONE;
