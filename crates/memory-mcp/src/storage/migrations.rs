//! Migration management for SurrealDB schema.
//!
//! Owns the migration catalog (scripts, checksums, validation) and the
//! migration runtime (lease protocol, schema postconditions, tolerance
//! rules). The runtime runs against [`SurrealDbClient`] through a narrow
//! seam: the client exposes script execution, template dimension, and
//! logging; the [`DbClient`] trait supplies record operations (C5).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::logging::LogLevel;
use crate::service::MemoryError;
use crate::storage::DbClient;
use crate::storage::client::SurrealDbClient;

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
        MigrationScript {
            file_name: "039_filesystem_ingestion.surql",
            sql: include_str!("../../migrations/039_filesystem_ingestion.surql"),
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

// ─── Migration runtime (C5: moved from client.rs) ─────────────────────

const INITIAL_SCHEMA_TABLES: &[&str] = &[
    "episode",
    "entity",
    "fact",
    "edge",
    "community",
    "event_log",
    "task",
    "script_migration",
];

const INITIAL_SCHEMA_FIELDS: &[&str] = &[
    "episode_id",
    "source_type",
    "source_id",
    "content",
    "t_ref",
    "t_ingested",
    "status",
    "archived_at",
    "scope",
    "visibility_scope",
    "policy_tags",
    "entity_id",
    "entity_type",
    "canonical_name",
    "canonical_name_normalized",
    "aliases",
    "fact_id",
    "fact_type",
    "quote",
    "source_episode",
    "t_valid",
    "t_invalid",
    "t_invalid_ingested",
    "confidence",
    "entity_links",
    "embedding",
    "provenance",
    "edge_id",
    "in",
    "relation",
    "out",
    "strength",
    "community_id",
    "member_entities",
    "summary",
    "updated_at",
    "ts",
    "op",
    "args",
    "result",
    "access",
    "transport",
    "content_type",
    "session_vars",
    "title",
    "due_date",
    "script_name",
    "executed_at",
    "checksum",
];

const INITIAL_SCHEMA_ANALYZERS: &[&str] = &["memory_fts"];

const INITIAL_SCHEMA_INDEXES: &[&str] = &[
    "episode_source_id",
    "entity_canonical_name",
    "entity_canonical_name_normalized",
    "entity_aliases",
    "fact_content_search",
    "fact_embedding_hnsw",
    "community_summary_search",
    "edge_relation",
    "edge_in",
    "edge_out",
    "community_members",
];

const INITIAL_SCHEMA_FLEXIBLE_COMPATIBILITY_ERROR: &str =
    "An error occurred: FLEXIBLE can only be used in SCHEMAFULL tables";
const MIGRATION_RUNNER_STATE_FILE: &str = "036_migration_runner_state.surql";
const MIGRATION_LEASE_SECS: i64 = 30;
const MIGRATION_WAIT_SECS: u64 = 5;
const MIGRATION_POLL_INTERVAL_MS: u64 = 100;

const EXPECTED_SCHEMA_TABLES: &[&str] = &[
    "episode",
    "entity",
    "fact",
    "edge",
    "community",
    "event_log",
    "task",
    "script_migration",
    "query_log",
    "embedding_state",
    "embedding_job",
    "triple",
    "memory_event",
    "event_projection_job",
    "memory_capture_audit",
    "procedure_candidate",
    "claim",
    "claim_relation",
    "claim_job",
    "claim_key_alias",
    "claim_policy",
    "entity_extraction_projection",
];

const EXPECTED_SCHEMA_ANALYZERS: &[&str] = &["memory_fts", "memory_fts_ru"];

/// Runs the full migration sequence for one namespace.
///
/// The caller ([`SurrealDbClient::apply_migrations_impl`]) has already
/// enforced the Active Namespace gate.
pub(crate) async fn run_migrations(
    client: &SurrealDbClient,
    namespace: &str,
) -> Result<(), MemoryError> {
    let initial_schema = render_sql_template(
        include_str!("../../migrations/__Initial.surql"),
        client.migration_embedding_dimension(),
    );

    // Initial migration may fail with "table already exists" if database was not cleanly shut down
    // or if tables were created by a previous version. We tolerate this error for idempotency.
    match client
        .execute_migration_script(&initial_schema, namespace)
        .await
    {
        Ok(()) => {}
        Err(MemoryError::Storage(err_msg)) if is_tolerable_initial_schema_error(&err_msg) => {
            client.migration_logger().log(
                HashMap::from([(
                    "op".to_string(),
                    Value::String("schema.init.compatibility_conflicts".to_string()),
                )]),
                LogLevel::Debug,
            );
        }
        Err(e) => return Err(e),
    }

    // Migration 036 adds the durable runner fields used by all other
    // versioned migrations. Apply its idempotent DDL first; its ledger row
    // is still recorded in the normal ordered loop below.
    ensure_migration_runner_schema(client, namespace).await?;

    for migration in versioned_migrations() {
        apply_versioned_migration(client, namespace, migration).await?;
    }

    verify_schema_postconditions(client, namespace).await?;

    client.migration_logger().log(
        HashMap::from([
            ("op".to_string(), Value::String("schema.init".to_string())),
            (
                "namespace".to_string(),
                Value::String(namespace.to_string()),
            ),
        ]),
        LogLevel::Info,
    );

    Ok(())
}

async fn ensure_migration_runner_schema(
    client: &SurrealDbClient,
    namespace: &str,
) -> Result<(), MemoryError> {
    let migration = versioned_migrations()
        .iter()
        .find(|migration| migration.file_name == MIGRATION_RUNNER_STATE_FILE)
        .ok_or_else(|| {
            MemoryError::Storage(format!(
                "migration runner bootstrap `{MIGRATION_RUNNER_STATE_FILE}` is not registered"
            ))
        })?;
    let rendered_sql = render_sql_template(migration.sql, client.migration_embedding_dimension());
    client
        .execute_migration_script(&rendered_sql, namespace)
        .await
}

async fn verify_schema_postconditions(
    client: &SurrealDbClient,
    namespace: &str,
) -> Result<(), MemoryError> {
    let db_info = client.query("INFO FOR DB", None, namespace).await?;
    let db_info = first_info_object(&db_info, "database")?;
    let tables = info_names(db_info.get("tables"), "tables")?;
    let analyzers = info_names(db_info.get("analyzers"), "analyzers")?;

    for table in EXPECTED_SCHEMA_TABLES {
        if !tables.contains(*table) {
            return Err(MemoryError::Storage(format!(
                "schema readiness failed in namespace `{namespace}`: missing table `{table}`"
            )));
        }
    }
    for analyzer in EXPECTED_SCHEMA_ANALYZERS {
        if !analyzers.contains(*analyzer) {
            return Err(MemoryError::Storage(format!(
                "schema readiness failed in namespace `{namespace}`: missing analyzer `{analyzer}`"
            )));
        }
    }

    for table in EXPECTED_SCHEMA_TABLES {
        let table_info = client
            .query(&format!("INFO FOR TABLE {table}"), None, namespace)
            .await?;
        let table_info = first_info_object(&table_info, table)?;
        let fields = info_names(table_info.get("fields"), "fields")?;
        for field in required_schema_fields(table) {
            if !fields.contains(*field) {
                return Err(MemoryError::Storage(format!(
                    "schema readiness failed in namespace `{namespace}`: missing field `{field}` on table `{table}`"
                )));
            }
        }
        let indexes = info_names(table_info.get("indexes"), "indexes")?;
        for index in required_schema_indexes(table) {
            if !indexes.contains(*index) {
                return Err(MemoryError::Storage(format!(
                    "schema readiness failed in namespace `{namespace}`: missing index `{index}` on table `{table}`"
                )));
            }
        }
    }

    Ok(())
}

async fn apply_versioned_migration(
    client: &SurrealDbClient,
    namespace: &str,
    migration: &MigrationScript,
) -> Result<(), MemoryError> {
    let record_id = migration_record_id(migration.file_name);
    let rendered_sql = render_sql_template(migration.sql, client.migration_embedding_dimension());
    let checksum = migration_checksum(&rendered_sql);

    // The runner-state DDL was bootstrapped before this loop. It is safe to
    // execute repeatedly, and its ledger record is created through the same
    // lease path as every other migration.
    let owner = migration_owner();
    let deadline = Instant::now() + Duration::from_secs(MIGRATION_WAIT_SECS);
    loop {
        if let Some(existing) = client.select_one(&record_id, namespace).await? {
            validate_applied_migration_compatibility(&existing, migration.file_name, &checksum)?;
            match migration_status(&existing) {
                Some("applied") | None => return Ok(()),
                Some("applying") if migration_lease_is_active(&existing) => {
                    if Instant::now() >= deadline {
                        return Err(MemoryError::Storage(format!(
                            "migration `{}` is already being applied in namespace `{namespace}`; waited {}s",
                            migration.file_name, MIGRATION_WAIT_SECS
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(MIGRATION_POLL_INTERVAL_MS)).await;
                    continue;
                }
                Some("failed") | Some("applying") => {
                    if !claim_existing_migration(client, &record_id, &owner, namespace).await? {
                        continue;
                    }
                }
                Some(status) => {
                    return Err(MemoryError::Storage(format!(
                        "migration `{}` has unsupported ledger status `{status}`",
                        migration.file_name
                    )));
                }
            }
        } else if create_migration_lease(
            client,
            &record_id,
            migration.file_name,
            &checksum,
            &owner,
            namespace,
        )
        .await?
        {
            break;
        }

        if Instant::now() >= deadline {
            return Err(MemoryError::Storage(format!(
                "could not reserve migration `{}` in namespace `{namespace}`; waited {}s",
                migration.file_name, MIGRATION_WAIT_SECS
            )));
        }
        tokio::time::sleep(Duration::from_millis(MIGRATION_POLL_INTERVAL_MS)).await;
    }

    let execution = if migration_has_statements(&rendered_sql) {
        client
            .execute_migration_script(&rendered_sql, namespace)
            .await
    } else {
        Ok(())
    };
    if let Err(error) = execution {
        let _ = mark_migration_failed(client, &record_id, &error.to_string(), namespace).await;
        return Err(error);
    }

    mark_migration_applied(
        client,
        &record_id,
        migration.file_name,
        &checksum,
        namespace,
    )
    .await
}

async fn create_migration_lease(
    client: &SurrealDbClient,
    record_id: &str,
    file_name: &str,
    checksum: &str,
    owner: &str,
    namespace: &str,
) -> Result<bool, MemoryError> {
    let body = migration_record_body(record_id)?;
    let now = chrono::Utc::now();
    let sql = format!(
        "CREATE script_migration:⟨{body}⟩ SET script_name = $script_name, checksum = $checksum, status = 'applying', owner = $owner, lease_expires_at = type::datetime($lease_expires_at), started_at = type::datetime($started_at), executed_at = type::datetime($executed_at) RETURN *"
    );
    match client
        .query(
            &sql,
            Some(json!({
                "script_name": file_name,
                "checksum": checksum,
                "owner": owner,
                "lease_expires_at": (now + chrono::Duration::seconds(MIGRATION_LEASE_SECS)).to_rfc3339(),
                "started_at": now.to_rfc3339(),
                "executed_at": now.to_rfc3339(),
            })),
            namespace,
        )
        .await
    {
        Ok(_) => Ok(true),
        Err(MemoryError::Storage(message))
            if super::client::is_record_already_exists_error(&message) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

async fn claim_existing_migration(
    client: &SurrealDbClient,
    record_id: &str,
    owner: &str,
    namespace: &str,
) -> Result<bool, MemoryError> {
    let Some(body) = record_id.strip_prefix("script_migration:") else {
        return Err(MemoryError::Storage(format!(
            "invalid migration ledger id `{record_id}`"
        )));
    };
    let sql = format!(
        "UPDATE script_migration:⟨{body}⟩ SET status = 'applying', owner = $owner, lease_expires_at = type::datetime($lease_expires_at), started_at = type::datetime($started_at), last_error = NONE WHERE status != 'applying' OR lease_expires_at IS NONE OR lease_expires_at <= time::now() RETURN AFTER"
    );
    let now = chrono::Utc::now();
    let result = client
        .query(
            &sql,
            Some(json!({
                "owner": owner,
                "lease_expires_at": (now + chrono::Duration::seconds(MIGRATION_LEASE_SECS)).to_rfc3339(),
                "started_at": now.to_rfc3339(),
            })),
            namespace,
        )
        .await?;
    Ok(!result.as_array().is_none_or(Vec::is_empty))
}

async fn mark_migration_failed(
    client: &SurrealDbClient,
    record_id: &str,
    error: &str,
    namespace: &str,
) -> Result<(), MemoryError> {
    client
        .update(
            record_id,
            json!({
                "status": "failed",
                "owner": Value::Null,
                "lease_expires_at": Value::Null,
                "last_error": error,
            }),
            namespace,
        )
        .await
        .map(|_| ())
}

async fn mark_migration_applied(
    client: &SurrealDbClient,
    record_id: &str,
    file_name: &str,
    checksum: &str,
    namespace: &str,
) -> Result<(), MemoryError> {
    let body = migration_record_body(record_id)?;
    let sql = format!(
        "UPDATE script_migration:⟨{body}⟩ SET script_name = $script_name, checksum = $checksum, status = 'applied', owner = NONE, lease_expires_at = NONE, last_error = NONE, executed_at = type::datetime($executed_at) RETURN *"
    );
    client
        .query(
            &sql,
            Some(json!({
                "script_name": file_name,
                "checksum": checksum,
                "executed_at": chrono::Utc::now().to_rfc3339(),
            })),
            namespace,
        )
        .await
        .map(|_| ())
}

fn required_schema_fields(table: &str) -> &'static [&'static str] {
    match table {
        "episode" => &[
            "episode_id",
            "source_type",
            "source_id",
            "content",
            "t_ref",
            "t_ingested",
            "policy_tags",
        ],
        "entity" => &[
            "entity_id",
            "entity_type",
            "canonical_name",
            "canonical_name_normalized",
            "aliases",
        ],
        "fact" => &[
            "fact_id",
            "fact_type",
            "content",
            "quote",
            "source_episode",
            "t_valid",
            "t_ingested",
            "confidence",
            "entity_links",
            "policy_tags",
            "provenance",
            "index_keys",
            "access_count",
            "last_accessed",
        ],
        "edge" => &[
            "edge_id",
            "in",
            "relation",
            "out",
            "strength",
            "confidence",
            "provenance",
            "t_valid",
            "t_ingested",
            "origin",
        ],
        "community" => &["community_id", "member_entities", "summary", "updated_at"],
        "event_log" => &[
            "ts",
            "op",
            "args",
            "result",
            "access",
            "transport",
            "content_type",
            "session_vars",
        ],
        "task" => &["status", "title", "due_date"],
        "script_migration" => &[
            "script_name",
            "executed_at",
            "checksum",
            "status",
            "owner",
            "lease_expires_at",
            "started_at",
            "last_error",
        ],
        "query_log" => &[
            "query_log_id",
            "logged_at",
            "query",
            "view_mode",
            "result_count",
            "latency_ms",
            "cache_hit",
        ],
        "embedding_state" => &["status", "updated_at"],
        "embedding_job" => &[
            "job_id",
            "status",
            "target_signature",
            "provider",
            "dimension",
            "namespaces",
            "requested_at",
            "total_facts",
            "processed_facts",
            "succeeded_facts",
            "failed_facts",
            "namespace_progress",
        ],
        "triple" => &[
            "namespace",
            "subject",
            "predicate",
            "object",
            "confidence",
            "source_fact_id",
            "t_ingested",
        ],
        "memory_event" => &[
            "event_id",
            "event_kind",
            "task_fingerprint",
            "disposition",
            "trust_class",
            "origin_kind",
            "created_at",
        ],
        "event_projection_job" => &[
            "job_id",
            "event_id",
            "status",
            "attempts",
            "max_attempts",
            "origin_kind",
            "created_at",
        ],
        "memory_capture_audit" => &[
            "audit_id",
            "event_id",
            "content_hash",
            "content_byte_len",
            "disposition",
            "reason_codes",
            "created_at",
        ],
        "procedure_candidate" => &[
            "candidate_id",
            "namespace",
            "task_fingerprint",
            "normalized_task",
            "status",
            "trust_floor",
            "success_count",
            "failure_count",
            "evidence_count",
            "origin_kind",
            "created_at",
            "updated_at",
        ],
        "claim" => &[
            "claim_id",
            "namespace",
            "source_fact_id",
            "source_episode_id",
            "policy_tags",
            "access_policy_fingerprint",
            "schema_family",
            "schema_version",
            "subject",
            "subject_key",
            "comparison_key",
            "comparison_key_hash",
            "qualifiers",
            "qualifier_hash",
            "slot_fingerprint",
            "value",
            "cardinality",
            "observed_at",
            "validity_source",
            "derivation",
            "extractor_fingerprint",
            "t_ingested",
            "identity_version",
        ],
        "claim_relation" => &[
            "claim_relation_id",
            "left_claim_id",
            "right_claim_id",
            "pair_fingerprint",
            "outcome",
            "schema_family",
            "schema_version",
            "left_fact_id",
            "right_fact_id",
            "reason_code",
            "evidence",
            "evaluator_version",
            "context_fingerprint",
            "evaluated_at",
            "policy_tags",
            "t_ingested",
        ],
        "claim_job" => &[
            "job_id",
            "kind",
            "namespace",
            "extractor_fingerprint",
            "status",
            "cursor",
            "lease_owner",
            "lease_expires_at",
            "processed",
            "succeeded",
            "skipped",
            "failed",
            "retry_count",
            "created_at",
            "updated_at",
        ],
        "claim_key_alias" => &[
            "alias_id",
            "schema_family",
            "canonical_key_hash",
            "alias_key_hash",
            "registry_version",
            "confirmed_by",
            "t_ingested",
        ],
        "claim_policy" => &[
            "policy_id",
            "schema_family",
            "schema_version",
            "policy_fingerprint",
            "definition",
            "t_ingested",
        ],
        "entity_extraction_projection" => &[
            "episode_id",
            "scope",
            "t_ingested",
            "t_created",
            "fingerprint",
            "entity_ids",
        ],
        _ => &[],
    }
}

fn required_schema_indexes(table: &str) -> &'static [&'static str] {
    match table {
        "episode" => &["episode_source_id", "episode_project"],
        "entity" => &[
            "entity_canonical_name",
            "entity_canonical_name_normalized",
            "entity_aliases",
        ],
        "fact" => &[
            "fact_content_search",
            "fact_embedding_hnsw",
            "fact_index_keys_search",
            "fact_project",
            "fact_project_type",
            "fact_claim_backfill_cursor_idx",
        ],
        "edge" => &[
            "edge_relation",
            "edge_in",
            "edge_out",
            "edge_from_to_idx",
            "edge_temporal_idx",
        ],
        "community" => &["community_summary_search", "community_members"],
        "query_log" => &[
            "query_log_scope_logged_at",
            "query_log_logged_at",
            "query_log_scope_resolved_view_logged_at",
        ],
        "triple" => &[
            "triple_subject_idx",
            "triple_predicate_idx",
            "triple_spo_idx",
        ],
        "memory_event" => &[
            "memory_event_id",
            "memory_event_session_kind",
            "memory_event_disposition",
        ],
        "event_projection_job" => &[
            "event_projection_job_id",
            "event_projection_job_status",
            "event_projection_job_lease",
            "event_projection_job_event",
        ],
        "memory_capture_audit" => &[
            "memory_capture_audit_id",
            "memory_capture_audit_event",
            "memory_capture_audit_disposition",
        ],
        "procedure_candidate" => &[
            "procedure_candidate_id",
            "procedure_candidate_scope_project",
            "procedure_candidate_status",
        ],
        "claim" => &["claim_slot_cursor_idx", "claim_source_projection_idx"],
        "claim_relation" => &[
            "claim_relation_left_active_idx",
            "claim_relation_right_active_idx",
            "claim_relation_context_idx",
            "claim_relation_left_fact_active_idx",
            "claim_relation_right_fact_active_idx",
            "claim_relation_schema_outcome_active_idx",
        ],
        "claim_job" => &["claim_job_lease_idx", "claim_job_fact_idx"],
        "claim_key_alias" => &["claim_alias_lookup_idx"],
        "claim_policy" => &["claim_policy_lookup_idx"],
        "entity_extraction_projection" => &[
            "entity_extraction_projection_episode_idx",
            "entity_extraction_projection_ingested_idx",
        ],
        _ => &[],
    }
}

fn first_info_object<'a>(
    value: &'a Value,
    resource: &str,
) -> Result<&'a serde_json::Map<String, Value>, MemoryError> {
    let object = match value {
        Value::Array(values) => values.first(),
        Value::Object(_) => Some(value),
        _ => None,
    }
    .and_then(Value::as_object)
    .ok_or_else(|| {
        MemoryError::Storage(format!(
            "schema readiness failed: INFO FOR {resource} returned no object"
        ))
    })?;
    Ok(object)
}

fn info_names(
    value: Option<&Value>,
    resource: &str,
) -> Result<std::collections::HashSet<String>, MemoryError> {
    value
        .and_then(Value::as_object)
        .map(|object| object.keys().cloned().collect())
        .ok_or_else(|| {
            MemoryError::Storage(format!(
                "schema readiness failed: INFO FOR {resource} returned no `{resource}` map"
            ))
        })
}

fn migration_record_body(record_id: &str) -> Result<&str, MemoryError> {
    let body = record_id.strip_prefix("script_migration:").ok_or_else(|| {
        MemoryError::Storage(format!("invalid migration ledger id `{record_id}`"))
    })?;
    if body.is_empty()
        || !body
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(MemoryError::Storage(format!(
            "invalid migration ledger id `{record_id}`"
        )));
    }
    Ok(body)
}

fn migration_owner() -> String {
    format!(
        "{}:{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("migration")
    )
}

fn migration_status(record: &Value) -> Option<&str> {
    record.get("status").and_then(Value::as_str)
}

fn migration_lease_is_active(record: &Value) -> bool {
    let Some(lease) = record
        .get("lease_expires_at")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    else {
        return false;
    };
    lease > chrono::Utc::now()
}

fn validate_applied_migration_compatibility(
    existing: &Value,
    expected_file_name: &str,
    expected_checksum: &str,
) -> Result<(), MemoryError> {
    let status = migration_status(existing);
    if matches!(status, Some("applying") | Some("failed")) {
        return validate_migration_identity(existing, expected_file_name, expected_checksum);
    }

    validate_applied_migration(existing, expected_file_name, expected_checksum)
}

fn is_tolerable_initial_schema_error(message: &str) -> bool {
    let details = message
        .strip_prefix("SurrealDB query statement errors:\n")
        .unwrap_or(message);

    !details.is_empty()
        && details.lines().all(|line| {
            let error = line.split_once(": ").map_or(line, |(_, error)| error);
            is_tolerable_initial_schema_conflict(error)
        })
}

fn is_tolerable_initial_schema_conflict(error: &str) -> bool {
    if error == INITIAL_SCHEMA_FLEXIBLE_COMPATIBILITY_ERROR {
        return true;
    }

    let Some((kind, remainder)) = [
        ("table", error.strip_prefix("The table '")),
        ("field", error.strip_prefix("The field '")),
        ("analyzer", error.strip_prefix("The analyzer '")),
        ("index", error.strip_prefix("The index '")),
    ]
    .into_iter()
    .find_map(|(kind, remainder)| remainder.map(|remainder| (kind, remainder))) else {
        return false;
    };

    let Some(name) = remainder.strip_suffix("' already exists") else {
        return false;
    };

    match kind {
        "table" => INITIAL_SCHEMA_TABLES.contains(&name),
        "field" => INITIAL_SCHEMA_FIELDS.contains(&name),
        "analyzer" => INITIAL_SCHEMA_ANALYZERS.contains(&name),
        "index" => INITIAL_SCHEMA_INDEXES.contains(&name),
        _ => false,
    }
}

fn render_sql_template(template: &str, embedding_dimension: usize) -> String {
    template.replace(
        crate::storage::fact_embedding_dimension_placeholder(),
        &embedding_dimension.to_string(),
    )
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
    fn versioned_migrations_includes_039_filesystem_ingestion() {
        assert!(
            versioned_migrations()
                .iter()
                .any(|migration| { migration.file_name == "039_filesystem_ingestion.surql" })
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

    #[test]
    fn migration_compatibility_allows_recovery_without_executed_at() {
        for status in ["applying", "failed"] {
            let existing = serde_json::json!({
                "script_name": "032_scope_free_active_namespace_expand.surql",
                "checksum": "expected-checksum",
                "status": status
            });

            validate_applied_migration_compatibility(
                &existing,
                "032_scope_free_active_namespace_expand.surql",
                "expected-checksum",
            )
            .expect("recoverable migration records do not need executed_at");
        }
    }

    #[test]
    fn migration_compatibility_rejects_changed_recovery_record() {
        let existing = serde_json::json!({
            "script_name": "032_scope_free_active_namespace_expand.surql",
            "checksum": "expected-checksum",
            "status": "failed"
        });

        let error = validate_applied_migration_compatibility(
            &existing,
            "032_scope_free_active_namespace_expand.surql",
            "different-checksum",
        )
        .expect_err("recovery must not bypass checksum validation");
        assert!(error.to_string().contains("modified"));
    }

    #[test]
    fn migration_lease_activity_is_conservative() {
        let future = (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
        let past = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();

        assert!(migration_lease_is_active(&serde_json::json!({
            "lease_expires_at": future
        })));
        assert!(!migration_lease_is_active(&serde_json::json!({
            "lease_expires_at": past
        })));
        assert!(!migration_lease_is_active(&serde_json::json!({
            "lease_expires_at": "not-a-datetime"
        })));
        assert!(!migration_lease_is_active(&serde_json::json!({})));
    }

    #[test]
    fn initial_schema_tolerates_known_idempotent_definition_conflicts() {
        let message = [
            "statement 0: The table 'episode' already exists",
            "statement 1: The field 'episode_id' already exists",
            "statement 2: The analyzer 'memory_fts' already exists",
            "statement 3: The index 'fact_content_search' already exists",
            "statement 4: An error occurred: FLEXIBLE can only be used in SCHEMAFULL tables",
        ]
        .join("\n");
        let message = format!("SurrealDB query statement errors:\n{message}");

        for line in message.lines().skip(1) {
            let error = line.split_once(": ").map_or(line, |(_, error)| error);
            assert!(
                is_tolerable_initial_schema_conflict(error),
                "unexpectedly rejected known conflict: {error:?}"
            );
        }
        assert!(is_tolerable_initial_schema_error(&message));
        assert!(is_tolerable_initial_schema_error(
            "The table 'episode' already exists"
        ));
    }

    #[test]
    fn initial_schema_rejects_unknown_or_mixed_definition_errors() {
        assert!(!is_tolerable_initial_schema_error(
            "SurrealDB query statement errors:\\nstatement 0: The table 'episode' already exists\\nstatement 1: analyzer error"
        ));
        assert!(!is_tolerable_initial_schema_error(
            "The table 'future_table' already exists"
        ));
        assert!(!is_tolerable_initial_schema_error(
            "The field 'future_field' already exists"
        ));
        assert!(!is_tolerable_initial_schema_error(
            "The analyzer 'future_analyzer' already exists"
        ));
        assert!(!is_tolerable_initial_schema_error(
            "The index 'future_index' already exists"
        ));
    }

    #[tokio::test]
    async fn schema_postconditions_reject_missing_required_resources() {
        let client = SurrealDbClient::connect_in_memory("test_db", "test", "warn")
            .await
            .expect("in-memory db");

        client
            .query(
                "DEFINE TABLE episode SCHEMAFULL; DEFINE FIELD episode_id ON episode TYPE string;",
                None,
                "test",
            )
            .await
            .expect("seed partial schema");

        let error = verify_schema_postconditions(&client, "test")
            .await
            .expect_err("partial schema must fail readiness");
        let message = error.to_string();
        assert!(message.contains("missing table"));
        assert!(message.contains("entity"));
    }
}
