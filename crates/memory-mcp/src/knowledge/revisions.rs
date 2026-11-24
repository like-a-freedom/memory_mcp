//! Transaction metadata for server-owned knowledge visibility boundaries.
//!
//! The bounded context still owns each record and its mutation. This adapter
//! only allocates the database-local revision, stamps a caller-selected record
//! with it, and appends a content-free ledger row in the same transaction.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, queries::build_create_query};

#[derive(Debug, Clone, Copy)]
pub(crate) enum KnowledgeRevisionKind {
    Episode,
    Fact,
    Claim,
    ClaimRelation,
    Edge,
    Triple,
}

/// Metadata carried alongside a trusted mutation when its commit is also
/// published to the HTTP tenant outbox.
#[derive(Debug, Clone)]
pub(crate) struct KnowledgeRevisionMetadata {
    pub(crate) idempotency_key: String,
    pub(crate) operation_kind: String,
    pub(crate) record_id: String,
    pub(crate) affected_record_ids_sql: String,
}

impl KnowledgeRevisionKind {
    fn operation_name(self) -> &'static str {
        match self {
            Self::Episode => "episode_created",
            Self::Fact => "fact_created",
            Self::Claim => "claim_created",
            Self::ClaimRelation => "claim_relation_created",
            Self::Edge => "edge_created",
            Self::Triple => "triple_created",
        }
    }

    fn expected_table(self) -> &'static str {
        match self {
            Self::Episode => "episode",
            Self::Fact => "fact",
            Self::Claim => "claim",
            Self::ClaimRelation => "claim_relation",
            Self::Edge => "edge",
            Self::Triple => "triple",
        }
    }
}

pub(crate) async fn create(
    db: &BoundDbClient,
    record_id: &str,
    content: Value,
    temporal_fields: &[&str],
    kind: KnowledgeRevisionKind,
) -> Result<Value, MemoryError> {
    let (mutation_sql, variables, _revision) =
        prepare_create(record_id, content, temporal_fields, kind)?;
    let script = revision_transaction_script(&mutation_sql, "");
    db.transaction_query(&script, Some(variables)).await?;
    db.select_one(record_id).await?.ok_or_else(|| {
        MemoryError::Storage("revisioned knowledge write did not persist its record".into())
    })
}

pub(crate) async fn create_with_query(
    db: &BoundDbClient,
    record_id: &str,
    content: Value,
    create_sql: String,
    variables: Value,
    kind: KnowledgeRevisionKind,
) -> Result<Value, MemoryError> {
    let (mutation_sql, variables, _revision) =
        prepare_create_with_query(record_id, content, create_sql, variables, kind)?;
    let script = revision_transaction_script(&mutation_sql, "");
    db.transaction_query(&script, Some(variables)).await?;
    db.select_one(record_id).await?.ok_or_else(|| {
        MemoryError::Storage("revisioned knowledge write did not persist its record".into())
    })
}

pub(crate) fn prepare_create(
    record_id: &str,
    content: Value,
    temporal_fields: &[&str],
    kind: KnowledgeRevisionKind,
) -> Result<(String, Value, KnowledgeRevisionMetadata), MemoryError> {
    let (create_sql, variables) = build_create_query(record_id, content.clone(), temporal_fields);
    prepare_create_with_query(record_id, content, create_sql, variables, kind)
}

pub(crate) fn prepare_create_with_query(
    record_id: &str,
    content: Value,
    create_sql: String,
    variables: Value,
    kind: KnowledgeRevisionKind,
) -> Result<(String, Value, KnowledgeRevisionMetadata), MemoryError> {
    crate::storage::validate_record_id(record_id)?;
    let Some((table, _)) = record_id.split_once(':') else {
        return Err(MemoryError::Validation(
            "knowledge revision requires a record id".into(),
        ));
    };
    if table != kind.expected_table() {
        return Err(MemoryError::Validation(
            "knowledge revision kind does not match the record table".into(),
        ));
    }
    let Value::Object(mut fields) = content.clone() else {
        return Err(MemoryError::Validation(
            "revisioned knowledge records must be objects".into(),
        ));
    };
    for reserved in ["known_revision", "closed_revision"] {
        if fields.remove(reserved).is_some() {
            return Err(MemoryError::Validation(
                "knowledge revision fields are server-owned".into(),
            ));
        }
    }
    let create_sql = add_revision_assignment(&create_sql)?;
    let idempotency_key = idempotency_key(kind.operation_name(), record_id, &fields)?;
    let mut variables = variables.as_object().cloned().ok_or_else(|| {
        MemoryError::Validation("revisioned knowledge record bindings must be an object".into())
    })?;
    for reserved in [
        "__knowledge_revision",
        "__knowledge_revision_key",
        "__knowledge_revision_kind",
        "__knowledge_revision_record_id",
    ] {
        if variables.contains_key(reserved) {
            return Err(MemoryError::Validation(
                "knowledge revision binding name is reserved".into(),
            ));
        }
    }
    variables.insert(
        "__knowledge_revision_key".into(),
        Value::String(idempotency_key.clone()),
    );
    variables.insert(
        "__knowledge_revision_kind".into(),
        Value::String(kind.operation_name().into()),
    );
    variables.insert(
        "__knowledge_revision_record_id".into(),
        Value::String(record_id.into()),
    );

    Ok((
        create_sql,
        Value::Object(variables),
        KnowledgeRevisionMetadata {
            idempotency_key,
            operation_kind: kind.operation_name().to_string(),
            record_id: record_id.to_string(),
            affected_record_ids_sql: "[$__knowledge_revision_record_id]".into(),
        },
    ))
}

pub(crate) fn revision_transaction_script(mutation_sql: &str, after_sql: &str) -> String {
    revision_transaction_script_with_affected_ids(
        mutation_sql,
        after_sql,
        "[$__knowledge_revision_record_id]",
    )
}

pub(crate) fn revision_transaction_script_with_affected_ids(
    mutation_sql: &str,
    after_sql: &str,
    affected_record_ids_sql: &str,
) -> String {
    format!(
        "LET $__revision_exists = SELECT VALUE revision FROM knowledge_revision WHERE idempotency_key = $__knowledge_revision_key LIMIT 1; \
         IF array::len($__revision_exists) = 0 {{ \
             LET $__revision_clock = SELECT latest_revision, last_recorded_at FROM knowledge_revision_clock:active LIMIT 1; \
             IF array::len($__revision_clock) != 1 {{ THROW 'knowledge revision clock is missing'; }}; \
             LET $__knowledge_revision = $__revision_clock[0].latest_revision + 1; \
             LET $__now = time::now(); \
             LET $__recorded_at = IF $__now > $__revision_clock[0].last_recorded_at {{ $__now }} ELSE {{ $__revision_clock[0].last_recorded_at }}; \
             UPDATE knowledge_revision_clock:active SET latest_revision = $__knowledge_revision, last_recorded_at = $__recorded_at; \
             {mutation_sql}; \
             CREATE knowledge_revision SET revision = $__knowledge_revision, recorded_at = $__recorded_at, operation_kind = $__knowledge_revision_kind, affected_record_ids = {affected_record_ids_sql}, idempotency_key = $__knowledge_revision_key; \
             {after_sql}; \
         }}"
    )
}

pub(crate) fn prepare_fact_invalidation(
    fact_id: &str,
    t_invalid: chrono::DateTime<chrono::Utc>,
    reason: &str,
) -> Result<(String, Value, KnowledgeRevisionMetadata), MemoryError> {
    crate::storage::validate_record_id(fact_id)?;
    if !fact_id.starts_with("fact:") {
        return Err(MemoryError::Validation(
            "knowledge revision invalidation requires a fact record id".into(),
        ));
    }
    let timestamps = crate::knowledge::CloseTimestamps {
        t_invalid: Some(t_invalid),
        t_invalid_ingested: None,
    };
    let (sql, vars) = crate::knowledge::build_close_query(fact_id, &timestamps, Some(reason))?;
    let sql = add_close_revision_assignment(&sql)?;
    let sql = sql.replace(
        "RETURN NONE",
        "WHERE (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL) RETURN BEFORE",
    );
    let Value::Object(mut vars) = vars else {
        return Err(MemoryError::Validation(
            "fact invalidation bindings must be an object".into(),
        ));
    };
    for reserved in [
        "__knowledge_revision",
        "__knowledge_revision_key",
        "__knowledge_revision_kind",
        "__knowledge_revision_record_id",
    ] {
        if vars.contains_key(reserved) {
            return Err(MemoryError::Validation(
                "knowledge revision binding name is reserved".into(),
            ));
        }
    }
    let idempotency_key = idempotency_key("fact_invalidated", fact_id, &serde_json::Map::new())?;
    vars.insert(
        "__knowledge_revision_key".into(),
        Value::String(idempotency_key.clone()),
    );
    vars.insert(
        "__knowledge_revision_kind".into(),
        Value::String("fact_invalidated".into()),
    );
    vars.insert(
        "__knowledge_revision_record_id".into(),
        Value::String(fact_id.into()),
    );

    let mutation_sql = format!(
        "LET $__revision_claim_ids = (SELECT VALUE type::string(id) FROM claim WHERE source_fact_id = $__knowledge_revision_record_id AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)); \
         LET $__revision_relation_ids = (SELECT VALUE type::string(id) FROM claim_relation WHERE (left_fact_id = $__knowledge_revision_record_id OR right_fact_id = $__knowledge_revision_record_id) AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)); \
         LET $__revision_fact_closed = ({sql}); \
         IF array::len($__revision_fact_closed) = 0 {{ THROW 'fact to invalidate was not active'; }}; \
         UPDATE claim SET t_invalid_ingested = time::now(), closed_revision = $__knowledge_revision WHERE source_fact_id = $__knowledge_revision_record_id AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL); \
         UPDATE claim_relation SET t_invalid_ingested = time::now(), closed_revision = $__knowledge_revision WHERE (left_fact_id = $__knowledge_revision_record_id OR right_fact_id = $__knowledge_revision_record_id) AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)"
    );
    let affected_record_ids_sql = "array::concat([$__knowledge_revision_record_id], $__revision_claim_ids, $__revision_relation_ids)";
    Ok((
        mutation_sql,
        Value::Object(vars),
        KnowledgeRevisionMetadata {
            idempotency_key,
            operation_kind: "fact_invalidated".into(),
            record_id: fact_id.into(),
            affected_record_ids_sql: affected_record_ids_sql.into(),
        },
    ))
}

pub(crate) fn prepare_record_close(
    record_id: &str,
    timestamps: &crate::knowledge::CloseTimestamps,
    reason: Option<&str>,
) -> Result<(String, Value, KnowledgeRevisionMetadata), MemoryError> {
    crate::storage::validate_record_id(record_id)?;
    let (table, _) = record_id.split_once(':').ok_or_else(|| {
        MemoryError::Validation("knowledge revision close requires a record id".into())
    })?;
    if !matches!(table, "fact" | "edge" | "triple") {
        return Err(MemoryError::Validation(
            "knowledge revision close only supports facts, edges, and triples".into(),
        ));
    }
    let (sql, vars) = crate::knowledge::build_close_query(record_id, timestamps, reason)?;
    let sql = add_close_revision_assignment(&sql)?;
    let sql = sql.replace(
        "RETURN NONE",
        "WHERE (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL) RETURN BEFORE",
    );
    let Value::Object(mut vars) = vars else {
        return Err(MemoryError::Validation(
            "knowledge revision close bindings must be an object".into(),
        ));
    };
    for reserved in [
        "__knowledge_revision",
        "__knowledge_revision_key",
        "__knowledge_revision_kind",
        "__knowledge_revision_record_id",
    ] {
        if vars.contains_key(reserved) {
            return Err(MemoryError::Validation(
                "knowledge revision binding name is reserved".into(),
            ));
        }
    }
    let operation_kind = format!("{table}_closed");
    let idempotency_key = idempotency_key(&operation_kind, record_id, &serde_json::Map::new())?;
    vars.insert(
        "__knowledge_revision_key".into(),
        Value::String(idempotency_key.clone()),
    );
    vars.insert(
        "__knowledge_revision_kind".into(),
        Value::String(operation_kind.clone()),
    );
    vars.insert(
        "__knowledge_revision_record_id".into(),
        Value::String(record_id.into()),
    );
    Ok((
        sql,
        Value::Object(vars),
        KnowledgeRevisionMetadata {
            idempotency_key,
            operation_kind,
            record_id: record_id.into(),
            affected_record_ids_sql: "[$__knowledge_revision_record_id]".into(),
        },
    ))
}

fn add_close_revision_assignment(sql: &str) -> Result<String, MemoryError> {
    let Some(return_position) = sql.rfind(" RETURN NONE") else {
        return Err(MemoryError::Storage(
            "close query has no record result clause".into(),
        ));
    };
    let (prefix, suffix) = sql.split_at(return_position);
    Ok(format!(
        "{prefix}, closed_revision = $__knowledge_revision{suffix}"
    ))
}

fn add_revision_assignment(sql: &str) -> Result<String, MemoryError> {
    let Some(return_position) = sql.rfind(" RETURN *") else {
        return Err(MemoryError::Storage(
            "create query has no record result clause".into(),
        ));
    };
    let (prefix, suffix) = sql.split_at(return_position);
    if prefix.contains(" SET ") {
        Ok(format!(
            "{prefix}, known_revision = $__knowledge_revision{suffix}"
        ))
    } else {
        Ok(format!(
            "{prefix} SET known_revision = $__knowledge_revision{suffix}"
        ))
    }
}

fn idempotency_key(
    operation: &str,
    record_id: &str,
    content: &Map<String, Value>,
) -> Result<String, MemoryError> {
    let bytes = serde_json::to_vec(&(operation, record_id, content))
        .map_err(|_| MemoryError::Storage("knowledge revision identity failed".into()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
