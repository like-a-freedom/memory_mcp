-- Migration 027: Claim reconciliation tables and indexes.
--
-- Adds five append-only tables for the claim reconciliation pipeline:
-- claim, claim_relation, claim_job, claim_key_alias, claim_policy.
-- Also adds an invalidation_reason field to fact for future lifecycle use.

-- Add optional invalidation reason to existing fact table.
DEFINE FIELD invalidation_reason ON fact TYPE option<string>;

-- ─── Claim ────────────────────────────────────────────────────────────────────

DEFINE TABLE claim SCHEMAFULL;
DEFINE FIELD claim_id ON claim TYPE string;
DEFINE FIELD namespace ON claim TYPE string;
DEFINE FIELD source_fact_id ON claim TYPE string;
DEFINE FIELD source_episode_id ON claim TYPE string;
DEFINE FIELD scope ON claim TYPE string;
DEFINE FIELD project ON claim TYPE option<string>;
DEFINE FIELD project_identity ON claim TYPE string;
DEFINE FIELD policy_tags ON claim TYPE array;
DEFINE FIELD access_policy_fingerprint ON claim TYPE string;
DEFINE FIELD schema_family ON claim TYPE string;
DEFINE FIELD schema_version ON claim TYPE int;
DEFINE FIELD subject ON claim TYPE object FLEXIBLE;
DEFINE FIELD subject_key ON claim TYPE string;
DEFINE FIELD comparison_key ON claim TYPE object FLEXIBLE;
DEFINE FIELD comparison_key_hash ON claim TYPE string;
DEFINE FIELD qualifiers ON claim TYPE object FLEXIBLE;
DEFINE FIELD qualifier_hash ON claim TYPE string;
DEFINE FIELD slot_fingerprint ON claim TYPE string;
DEFINE FIELD value ON claim TYPE object FLEXIBLE;
DEFINE FIELD cardinality ON claim TYPE string;
DEFINE FIELD observed_at ON claim TYPE datetime;
DEFINE FIELD valid_from ON claim TYPE option<datetime>;
DEFINE FIELD valid_to ON claim TYPE option<datetime>;
DEFINE FIELD validity_source ON claim TYPE string;
DEFINE FIELD source_lineage ON claim TYPE option<string>;
DEFINE FIELD derivation ON claim TYPE object FLEXIBLE;
DEFINE FIELD extractor_fingerprint ON claim TYPE string;
DEFINE FIELD t_ingested ON claim TYPE datetime;
DEFINE FIELD t_invalid_ingested ON claim TYPE option<datetime>;

-- ─── Claim Relation ───────────────────────────────────────────────────────────

DEFINE TABLE claim_relation SCHEMAFULL;
DEFINE FIELD claim_relation_id ON claim_relation TYPE string;
DEFINE FIELD left_claim_id ON claim_relation TYPE string;
DEFINE FIELD right_claim_id ON claim_relation TYPE string;
DEFINE FIELD pair_fingerprint ON claim_relation TYPE string;
DEFINE FIELD outcome ON claim_relation TYPE string;
DEFINE FIELD predecessor_claim_id ON claim_relation TYPE option<string>;
DEFINE FIELD successor_claim_id ON claim_relation TYPE option<string>;
DEFINE FIELD reason_code ON claim_relation TYPE string;
DEFINE FIELD evidence ON claim_relation TYPE object FLEXIBLE;
DEFINE FIELD evaluator_version ON claim_relation TYPE string;
DEFINE FIELD context_fingerprint ON claim_relation TYPE string;
DEFINE FIELD evaluated_at ON claim_relation TYPE datetime;
DEFINE FIELD supersedes_relation_id ON claim_relation TYPE option<string>;
DEFINE FIELD scope ON claim_relation TYPE string;
DEFINE FIELD project ON claim_relation TYPE option<string>;
DEFINE FIELD policy_tags ON claim_relation TYPE array;
DEFINE FIELD t_ingested ON claim_relation TYPE datetime;
DEFINE FIELD t_invalid_ingested ON claim_relation TYPE option<datetime>;

-- ─── Claim Job ────────────────────────────────────────────────────────────────

DEFINE TABLE claim_job SCHEMAFULL;
DEFINE FIELD job_id ON claim_job TYPE string;
DEFINE FIELD kind ON claim_job TYPE string;
DEFINE FIELD namespace ON claim_job TYPE string;
DEFINE FIELD source_fact_id ON claim_job TYPE option<string>;
DEFINE FIELD claim_id ON claim_job TYPE option<string>;
DEFINE FIELD extractor_fingerprint ON claim_job TYPE string;
DEFINE FIELD evaluator_fingerprint ON claim_job TYPE option<string>;
DEFINE FIELD status ON claim_job TYPE string;
DEFINE FIELD cursor ON claim_job TYPE option<string>;
DEFINE FIELD lease_owner ON claim_job TYPE option<string>;
DEFINE FIELD lease_expires_at ON claim_job TYPE option<datetime>;
DEFINE FIELD processed ON claim_job TYPE int;
DEFINE FIELD succeeded ON claim_job TYPE int;
DEFINE FIELD skipped ON claim_job TYPE int;
DEFINE FIELD failed ON claim_job TYPE int;
DEFINE FIELD retry_count ON claim_job TYPE int;
DEFINE FIELD last_error ON claim_job TYPE option<string>;
DEFINE FIELD created_at ON claim_job TYPE datetime;
DEFINE FIELD started_at ON claim_job TYPE option<datetime>;
DEFINE FIELD updated_at ON claim_job TYPE datetime;
DEFINE FIELD completed_at ON claim_job TYPE option<datetime>;

-- ─── Claim Key Alias ──────────────────────────────────────────────────────────

DEFINE TABLE claim_key_alias SCHEMAFULL;
DEFINE FIELD alias_id ON claim_key_alias TYPE string;
DEFINE FIELD schema_family ON claim_key_alias TYPE string;
DEFINE FIELD canonical_key_hash ON claim_key_alias TYPE string;
DEFINE FIELD alias_key_hash ON claim_key_alias TYPE string;
DEFINE FIELD registry_version ON claim_key_alias TYPE string;
DEFINE FIELD confirmed_by ON claim_key_alias TYPE string;
DEFINE FIELD t_ingested ON claim_key_alias TYPE datetime;
DEFINE FIELD t_invalid_ingested ON claim_key_alias TYPE option<datetime>;

-- ─── Claim Policy ─────────────────────────────────────────────────────────────

DEFINE TABLE claim_policy SCHEMAFULL;
DEFINE FIELD policy_id ON claim_policy TYPE string;
DEFINE FIELD schema_family ON claim_policy TYPE string;
DEFINE FIELD schema_version ON claim_policy TYPE int;
DEFINE FIELD policy_fingerprint ON claim_policy TYPE string;
DEFINE FIELD definition ON claim_policy TYPE object FLEXIBLE;
DEFINE FIELD t_ingested ON claim_policy TYPE datetime;
DEFINE FIELD t_invalid_ingested ON claim_policy TYPE option<datetime>;

-- ─── Indexes ──────────────────────────────────────────────────────────────────

DEFINE INDEX claim_slot_cursor_idx ON claim COLUMNS slot_fingerprint, claim_id;
DEFINE INDEX claim_source_projection_idx ON claim COLUMNS source_fact_id, extractor_fingerprint;
DEFINE INDEX claim_relation_left_active_idx ON claim_relation COLUMNS left_claim_id, t_invalid_ingested;
DEFINE INDEX claim_relation_right_active_idx ON claim_relation COLUMNS right_claim_id, t_invalid_ingested;
DEFINE INDEX claim_relation_context_idx ON claim_relation COLUMNS pair_fingerprint, context_fingerprint, t_invalid_ingested;
DEFINE INDEX claim_job_lease_idx ON claim_job COLUMNS status, lease_expires_at, job_id;
DEFINE INDEX claim_job_fact_idx ON claim_job COLUMNS source_fact_id, extractor_fingerprint, kind;
DEFINE INDEX claim_alias_lookup_idx ON claim_key_alias COLUMNS schema_family, alias_key_hash, t_invalid_ingested;
DEFINE INDEX claim_policy_lookup_idx ON claim_policy COLUMNS schema_family, policy_fingerprint, t_invalid_ingested;
DEFINE INDEX fact_claim_backfill_cursor_idx ON fact COLUMNS fact_id;
