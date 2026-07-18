-- Migration 028: harden persisted claim relation lookups and metrics.
-- Adds fact-level lookup fields and schema-labeled relation counts.
-- Applies after 027; never edits earlier migrations.

DEFINE FIELD schema_family ON claim_relation TYPE option<string>;
DEFINE FIELD schema_version ON claim_relation TYPE option<int>;
DEFINE FIELD left_fact_id ON claim_relation TYPE option<string>;
DEFINE FIELD right_fact_id ON claim_relation TYPE option<string>;

DEFINE INDEX claim_relation_left_fact_active_idx
    ON claim_relation COLUMNS left_fact_id, t_invalid_ingested;
DEFINE INDEX claim_relation_right_fact_active_idx
    ON claim_relation COLUMNS right_fact_id, t_invalid_ingested;
DEFINE INDEX claim_relation_schema_outcome_active_idx
    ON claim_relation COLUMNS schema_family, outcome, t_invalid_ingested;
