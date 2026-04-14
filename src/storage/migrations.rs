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

    let applied_name = map
        .get("script_name")
        .and_then(json_string)
        .ok_or_else(|| {
            MemoryError::Storage("applied migration record missing script_name".to_string())
        })?;
    let applied_checksum = map.get("checksum").and_then(json_string).ok_or_else(|| {
        MemoryError::Storage("applied migration record missing checksum".to_string())
    })?;
    let executed_at = map
        .get("executed_at")
        .and_then(json_string)
        .ok_or_else(|| {
            MemoryError::Storage("applied migration record missing executed_at".to_string())
        })?;

    if applied_name != expected_file_name {
        return Err(MemoryError::ConfigInvalid(format!(
            "applied migration name mismatch for {expected_file_name}: found {applied_name}"
        )));
    }

    if applied_checksum != expected_checksum {
        return Err(MemoryError::ConfigInvalid(format!(
            "applied migration {expected_file_name} was modified after execution"
        )));
    }

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
