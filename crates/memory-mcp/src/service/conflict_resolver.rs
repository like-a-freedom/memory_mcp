//! Conflict resolution for singleton predicates.
//!
//! When a new triple is created with a singleton predicate (e.g., "works_at"),
//! any existing active triple with the same (subject, predicate) but a different
//! object is automatically invalidated via bi-temporal close.

use crate::service::entity::EntityService;
use crate::service::error::MemoryError;
use crate::service::triple_extractor::SemanticTriple;

/// Resolve conflicts for a newly created triple.
///
/// If the triple's predicate is a singleton, find any existing active triples
/// with the same (subject, predicate) but a different object and invalidate them.
pub async fn resolve_conflicts_for_triple(
    entity_service: &EntityService,
    new_triple: &SemanticTriple,
) -> Result<Vec<String>, MemoryError> {
    if !crate::service::triple_extractor::is_singleton_predicate(&new_triple.predicate) {
        return Ok(vec![]);
    }

    // Find conflicting triples: same (subject, predicate), different object.
    let conflicting = find_conflicting_triples(
        entity_service,
        &new_triple.subject,
        &new_triple.predicate,
        &new_triple.object,
    )
    .await?;

    if conflicting.is_empty() {
        return Ok(vec![]);
    }

    // Invalidate conflicting triples via bi-temporal close.
    let mut invalidated = Vec::with_capacity(conflicting.len());
    for triple_id in &conflicting {
        invalidate_triple(entity_service, triple_id).await?;
        invalidated.push(triple_id.clone());
    }

    Ok(invalidated)
}

/// Find active triples with the same (subject, predicate) but a different object.
async fn find_conflicting_triples(
    entity_service: &EntityService,
    subject: &str,
    predicate: &str,
    exclude_object: &str,
) -> Result<Vec<String>, MemoryError> {
    let sql = r#"
        SELECT id FROM triple
        WHERE subject = $subject
          AND predicate = $predicate
          AND object != $object
          AND t_invalid IS NONE
        LIMIT 10
    "#;
    let result = entity_service
        .query_triples(sql, subject, predicate, exclude_object)
        .await?;

    Ok(result
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_object()?
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Invalidate a triple via bi-temporal close: set both the valid-time end
/// (`t_invalid`) and the transaction-time end (`t_invalid_ingested`).
///
/// `t_invalid_ingested` MUST be set whenever `t_invalid` is closed, so the
/// audit trail records *when the system learned* the triple was superseded —
/// not just when it logically stopped being true. This mirrors the existing
/// fact/edge invalidation path in `lifecycle/decay.rs`.
async fn invalidate_triple(
    entity_service: &EntityService,
    triple_id: &str,
) -> Result<(), MemoryError> {
    let sql =
        "UPDATE type::record($id) SET t_invalid = time::now(), t_invalid_ingested = time::now()";
    entity_service.invalidate_triple_by_id(sql, triple_id).await
}
