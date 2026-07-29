//! Verifies that claim-related SurrealDB migration SQL is valid and
//! that the expected schema objects are declared.

/// Load migration SQL from the repository at compile time.
macro_rules! migration_sql {
    ($name:expr) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/", $name))
    };
}

#[test]
fn migration_029_declares_all_five_tables() {
    let sql = migration_sql!("029_claim_reconciliation.surql");
    assert!(sql.contains("DEFINE TABLE claim"), "claim table");
    assert!(
        sql.contains("DEFINE TABLE claim_relation"),
        "claim_relation"
    );
    assert!(sql.contains("DEFINE TABLE claim_job"), "claim_job");
    assert!(
        sql.contains("DEFINE TABLE claim_key_alias"),
        "claim_key_alias"
    );
    assert!(sql.contains("DEFINE TABLE claim_policy"), "claim_policy");
}

#[test]
fn migration_029_defines_expected_indexes() {
    let sql = migration_sql!("029_claim_reconciliation.surql");
    assert!(sql.contains("claim_slot_cursor_idx"));
    assert!(sql.contains("claim_source_projection_idx"));
    assert!(sql.contains("claim_job_lease_idx"));
    assert!(sql.contains("claim_job_fact_idx"));
    assert!(sql.contains("fact_claim_backfill_cursor_idx"));
}

#[test]
fn migration_029_adds_invalidation_reason_to_fact() {
    let sql = migration_sql!("029_claim_reconciliation.surql");
    assert!(sql.contains("invalidation_reason"));
    assert!(sql.contains("DEFINE FIELD invalidation_reason ON fact"));
}

#[test]
fn migration_030_defines_hardening_indexes() {
    let sql = migration_sql!("030_claim_reconciliation_hardening.surql");
    assert!(sql.contains("claim_relation_left_fact_active_idx"));
    assert!(sql.contains("claim_relation_right_fact_active_idx"));
    assert!(sql.contains("claim_relation_schema_outcome_active_idx"));
}

#[test]
fn migration_030_adds_relation_lookup_fields() {
    let sql = migration_sql!("030_claim_reconciliation_hardening.surql");
    assert!(sql.contains("schema_family"));
    assert!(sql.contains("schema_version"));
    assert!(sql.contains("left_fact_id"));
    assert!(sql.contains("right_fact_id"));
}

#[test]
fn migration_030_defines_fields_on_claim_relation() {
    let sql = migration_sql!("030_claim_reconciliation_hardening.surql");
    assert!(sql.contains("schema_family ON claim_relation"));
    assert!(sql.contains("schema_version ON claim_relation"));
    assert!(sql.contains("left_fact_id ON claim_relation"));
    assert!(sql.contains("right_fact_id ON claim_relation"));
}
