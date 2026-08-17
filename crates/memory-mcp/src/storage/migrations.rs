//! Migration management for SurrealDB schema.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::service::MemoryError;

#[derive(Debug, Clone, Copy)]
pub struct MigrationScript {
    pub file_name: &'static str,
    pub sql: &'static str,
}

pub fn versioned_migrations() -> &'static [MigrationScript] {
    &[
        MigrationScript {
            file_name: "006_simplified_search_redesign.surql",
            sql: include_str!("../../migrations/006_simplified_search_redesign.surql"),
        },
        MigrationScript {
            file_name: "007_episode_archival_fields.surql",
            sql: include_str!("../../migrations/007_episode_archival_fields.surql"),
        },
        MigrationScript {
            file_name: "008_fact_semantic_embeddings.surql",
            sql: include_str!("../../migrations/008_fact_semantic_embeddings.surql"),
        },
        MigrationScript {
            file_name: "009_adaptive_memory_alignment.surql",
            sql: include_str!("../../migrations/009_adaptive_memory_alignment.surql"),
        },
        MigrationScript {
            file_name: "010_coerce_t_ingested_to_datetime.surql",
            sql: include_str!("../../migrations/010_coerce_t_ingested_to_datetime.surql"),
        },
        MigrationScript {
            file_name: "016_project_tag.surql",
            sql: include_str!("../../migrations/016_project_tag.surql"),
        },
        MigrationScript {
            file_name: "017_edge_origin.surql",
            sql: include_str!("../../migrations/017_edge_origin.surql"),
        },
        MigrationScript {
            file_name: "018_query_log.surql",
            sql: include_str!("../../migrations/018_query_log.surql"),
        },
        MigrationScript {
            file_name: "019_embedding_rebuild_maintenance.surql",
            sql: include_str!("../../migrations/019_embedding_rebuild_maintenance.surql"),
        },
        MigrationScript {
            file_name: "020_embedding_job_namespace_progress_flexible.surql",
            sql: include_str!(
                "../../migrations/020_embedding_job_namespace_progress_flexible.surql"
            ),
        },
        MigrationScript {
            file_name: "021_query_log_retrieval_diagnostics.surql",
            sql: include_str!("../../migrations/021_query_log_retrieval_diagnostics.surql"),
        },
        MigrationScript {
            file_name: "022_structured_provenance.surql",
            sql: include_str!("../../migrations/022_structured_provenance.surql"),
        },
        MigrationScript {
            file_name: "023_edge_composite_indexes.surql",
            sql: include_str!("../../migrations/023_edge_composite_indexes.surql"),
        },
        MigrationScript {
            file_name: "024_triples.surql",
            sql: include_str!("../../migrations/024_triples.surql"),
        },
        MigrationScript {
            file_name: "025_cyrillic_fts.surql",
            sql: include_str!("../../migrations/025_cyrillic_fts.surql"),
        },
        MigrationScript {
            file_name: "026_cyrillic_fts_active.surql",
            sql: include_str!("../../migrations/026_cyrillic_fts_active.surql"),
        },
        MigrationScript {
            file_name: "027_agent_memory_lifecycle.surql",
            sql: include_str!("../../migrations/027_agent_memory_lifecycle.surql"),
        },
        MigrationScript {
            file_name: "028_procedural_memory.surql",
            sql: include_str!("../../migrations/028_procedural_memory.surql"),
        },
        MigrationScript {
            file_name: "029_claim_reconciliation.surql",
            sql: include_str!("../../migrations/029_claim_reconciliation.surql"),
        },
        MigrationScript {
            file_name: "030_claim_reconciliation_hardening.surql",
            sql: include_str!("../../migrations/030_claim_reconciliation_hardening.surql"),
        },
        MigrationScript {
            file_name: "031_entity_extraction_projection.surql",
            sql: include_str!("../../migrations/031_entity_extraction_projection.surql"),
        },
        MigrationScript {
            file_name: "032_scope_free_active_namespace_expand.surql",
            sql: include_str!("../../migrations/032_scope_free_active_namespace_expand.surql"),
        },
        MigrationScript {
            file_name: "033_claim_identity_version.surql",
            sql: include_str!("../../migrations/033_claim_identity_version.surql"),
        },
        MigrationScript {
            file_name: "034_edge_provenance_schema_fix.surql",
            sql: include_str!("../../migrations/034_edge_provenance_schema_fix.surql"),
        },
        MigrationScript {
            file_name: "035_claim_legacy_identity_optional.surql",
            sql: include_str!("../../migrations/035_claim_legacy_identity_optional.surql"),
        },
        MigrationScript {
            file_name: "036_migration_runner_state.surql",
            sql: include_str!("../../migrations/036_migration_runner_state.surql"),
        },
        MigrationScript {
            file_name: "037_triple_legacy_namespace_optional.surql",
            sql: include_str!("../../migrations/037_triple_legacy_namespace_optional.surql"),
        },
        MigrationScript {
            file_name: "038_claim_source_span.surql",
            sql: include_str!("../../migrations/038_claim_source_span.surql"),
        },
    ]
}

pub fn migration_record_id(file_name: &str) -> String {
    let slug = file_name
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' => character,
            _ => '_',
        })
        .collect::<String>();
    format!("script_migration:{slug}")
}

pub fn migration_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn migration_has_statements(sql: &str) -> bool {
    sql.lines()
        .map(str::trim)
        .any(|line| !line.is_empty() && !line.starts_with("--"))
}

fn is_dynamic_embedding_migration(file_name: &str) -> bool {
    matches!(file_name, "008_fact_semantic_embeddings.surql")
}

/// Validates the immutable identity fields shared by every migration-ledger state.
///
/// In-progress records deliberately do not require `executed_at`: they must be
/// recoverable after a process crash. The caller is responsible for applying the
/// state-specific timestamp requirements.
pub(crate) fn validate_migration_identity(
    existing: &Value,
    expected_file_name: &str,
    expected_checksum: &str,
) -> Result<(), MemoryError> {
    let Some(map) = existing.as_object() else {
        return Err(MemoryError::Storage(
            "stored migration bookkeeping record must be an object".to_string(),
        ));
    };

    let applied_name = map
        .get("script_name")
        .and_then(json_string)
        .ok_or_else(|| MemoryError::Storage("migration record missing script_name".to_string()))?;
    let applied_checksum = map
        .get("checksum")
        .and_then(json_string)
        .ok_or_else(|| MemoryError::Storage("migration record missing checksum".to_string()))?;

    if applied_name != expected_file_name {
        return Err(MemoryError::ConfigInvalid(format!(
            "migration name mismatch for {expected_file_name}: found {applied_name}"
        )));
    }

    if applied_checksum != expected_checksum && !is_dynamic_embedding_migration(expected_file_name)
    {
        return Err(MemoryError::ConfigInvalid(format!(
            "migration {expected_file_name} was modified after execution"
        )));
    }

    Ok(())
}

pub fn validate_applied_migration(
    existing: &Value,
    expected_file_name: &str,
    expected_checksum: &str,
) -> Result<(), MemoryError> {
    let Some(map) = existing.as_object() else {
        return Err(MemoryError::Storage(
            "stored migration bookkeeping record must be an object".to_string(),
        ));
    };

    validate_migration_identity(existing, expected_file_name, expected_checksum)?;

    let executed_at = map
        .get("executed_at")
        .and_then(json_string)
        .ok_or_else(|| {
            MemoryError::Storage("applied migration record missing executed_at".to_string())
        })?;

    if chrono::DateTime::parse_from_rfc3339(executed_at).is_err() {
        return Err(MemoryError::Storage(format!(
            "applied migration {expected_file_name} has invalid executed_at"
        )));
    }

    Ok(())
}

fn json_string(value: &Value) -> Option<&str> {
    value.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_migrations_includes_embedding_rebuild_maintenance() {
        assert!(
            versioned_migrations()
                .iter()
                .any(|migration| migration.file_name == "019_embedding_rebuild_maintenance.surql")
        );
        assert!(versioned_migrations().iter().any(|migration| {
            migration.file_name == "020_embedding_job_namespace_progress_flexible.surql"
        }));
        assert!(versioned_migrations().iter().any(|migration| {
            migration.file_name == "021_query_log_retrieval_diagnostics.surql"
        }));
    }

    #[test]
    fn versioned_migrations_includes_026_cyrillic_fts_active() {
        // 025 defines `memory_fts_ru` but binds no index to it; 026 is what
        // actually activates Russian stemming on the FTS indexes. Both must
        // be registered so the schema is query-correct for Cyrillic content.
        assert!(
            versioned_migrations()
                .iter()
                .any(|migration| migration.file_name == "026_cyrillic_fts_active.surql")
        );
    }

    #[test]
    fn versioned_migrations_includes_027_agent_memory_lifecycle() {
        let migrations = versioned_migrations();
        assert!(
            migrations
                .iter()
                .any(|migration| migration.file_name == "027_agent_memory_lifecycle.surql")
        );
    }

    #[test]
    fn versioned_migrations_includes_028_procedural_memory() {
        let migrations = versioned_migrations();
        assert!(
            migrations
                .iter()
                .any(|migration| migration.file_name == "028_procedural_memory.surql")
        );
    }

    #[test]
    fn versioned_migrations_includes_031_entity_extraction_projection() {
        let migrations = versioned_migrations();
        assert!(
            migrations
                .iter()
                .any(|migration| migration.file_name == "031_entity_extraction_projection.surql")
        );
    }

    #[test]
    fn versioned_migrations_includes_032_scope_free_expand() {
        let migrations = versioned_migrations();
        assert!(migrations.iter().any(|migration| {
            migration.file_name == "032_scope_free_active_namespace_expand.surql"
        }));
    }

    #[test]
    fn versioned_migrations_includes_edge_provenance_schema_fix() {
        assert!(
            versioned_migrations()
                .iter()
                .any(|migration| { migration.file_name == "034_edge_provenance_schema_fix.surql" })
        );
    }

    #[test]
    fn versioned_migrations_includes_optional_claim_legacy_identity() {
        assert!(versioned_migrations().iter().any(|migration| {
            migration.file_name == "035_claim_legacy_identity_optional.surql"
        }));
    }

    #[test]
    fn versioned_migrations_includes_migration_runner_state() {
        assert!(
            versioned_migrations()
                .iter()
                .any(|migration| { migration.file_name == "036_migration_runner_state.surql" })
        );
    }

    #[test]
    fn versioned_migrations_have_no_duplicate_file_names() {
        let mut names: Vec<&str> = versioned_migrations()
            .iter()
            .map(|migration| migration.file_name)
            .collect();
        names.sort_unstable();
        let initial_len = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            initial_len,
            "duplicate migration file names detected"
        );
    }

    #[test]
    fn validate_migration_identity_accepts_recoverable_in_progress_records() {
        for status in ["applying", "failed"] {
            let existing = serde_json::json!({
                "script_name": "032_scope_free_active_namespace_expand.surql",
                "checksum": "expected-checksum",
                "status": status
            });

            assert!(
                validate_migration_identity(
                    &existing,
                    "032_scope_free_active_namespace_expand.surql",
                    "expected-checksum"
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn validate_migration_identity_rejects_changed_name_or_checksum() {
        let existing = serde_json::json!({
            "script_name": "032_scope_free_active_namespace_expand.surql",
            "checksum": "expected-checksum",
            "status": "applying"
        });

        let name_error = validate_migration_identity(
            &existing,
            "033_claim_identity_version.surql",
            "expected-checksum",
        )
        .expect_err("a changed migration name must fail closed");
        assert!(name_error.to_string().contains("name mismatch"));

        let checksum_error = validate_migration_identity(
            &existing,
            "032_scope_free_active_namespace_expand.surql",
            "different-checksum",
        )
        .expect_err("a changed migration checksum must fail closed");
        assert!(checksum_error.to_string().contains("modified"));
    }

    #[test]
    fn validate_applied_migration_requires_executed_at() {
        let existing = serde_json::json!({
            "script_name": "032_scope_free_active_namespace_expand.surql",
            "checksum": "expected-checksum",
            "status": "applied"
        });

        let error = validate_applied_migration(
            &existing,
            "032_scope_free_active_namespace_expand.surql",
            "expected-checksum",
        )
        .expect_err("applied records need a valid completion timestamp");
        assert!(error.to_string().contains("executed_at"));
    }

    #[test]
    fn validate_applied_migration_allows_dynamic_embedding_checksum_drift_for_008() {
        let existing = serde_json::json!({
            "script_name": "008_fact_semantic_embeddings.surql",
            "checksum": "checksum-from-384-database",
            "executed_at": "2026-04-30T00:00:00Z"
        });

        let result = validate_applied_migration(
            &existing,
            "008_fact_semantic_embeddings.surql",
            "checksum-from-1536-render",
        );

        assert!(result.is_ok());
    }
}
