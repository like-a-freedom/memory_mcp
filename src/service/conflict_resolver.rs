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
    namespace: &str,
    new_triple: &SemanticTriple,
) -> Result<Vec<String>, MemoryError> {
    if !crate::service::triple_extractor::is_singleton_predicate(&new_triple.predicate) {
        return Ok(vec![]);
    }

    // Find conflicting triples: same (subject, predicate), different object.
    let conflicting = find_conflicting_triples(
        entity_service,
        namespace,
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
        invalidate_triple(entity_service, namespace, triple_id).await?;
        invalidated.push(triple_id.clone());
    }

    Ok(invalidated)
}

/// Find active triples with the same (subject, predicate) but a different object.
async fn find_conflicting_triples(
    entity_service: &EntityService,
    namespace: &str,
    subject: &str,
    predicate: &str,
    exclude_object: &str,
) -> Result<Vec<String>, MemoryError> {
    let sql = r#"
        SELECT id FROM triple
        WHERE namespace = $ns
          AND subject = $subject
          AND predicate = $predicate
          AND object != $object
        LIMIT 10
    "#;
    let result = entity_service
        .query_triples(sql, namespace, subject, predicate, exclude_object)
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

/// Invalidate a triple by setting t_invalid (bi-temporal close).
async fn invalidate_triple(
    entity_service: &EntityService,
    namespace: &str,
    triple_id: &str,
) -> Result<(), MemoryError> {
    let sql = "UPDATE type::thing($id) SET t_invalid = time::now()";
    entity_service
        .invalidate_triple_by_id(sql, namespace, triple_id)
        .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn singleton_predicates_are_recognized() {
        assert!(crate::service::triple_extractor::is_singleton_predicate(
            "works_at"
        ));
        assert!(crate::service::triple_extractor::is_singleton_predicate(
            "lives_in"
        ));
        assert!(crate::service::triple_extractor::is_singleton_predicate(
            "has_email"
        ));
    }

    #[test]
    fn non_singleton_predicates_are_not_recognized() {
        assert!(!crate::service::triple_extractor::is_singleton_predicate(
            "knows"
        ));
        assert!(!crate::service::triple_extractor::is_singleton_predicate(
            "visited"
        ));
        assert!(!crate::service::triple_extractor::is_singleton_predicate(
            "met"
        ));
    }
}
