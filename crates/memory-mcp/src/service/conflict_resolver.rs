//! Conflict resolution for singleton predicates.
//!
//! When a new triple is created with a singleton predicate (e.g., "works_at"),
//! any existing active triple with the same (subject, predicate) but a different
//! object is automatically invalidated via bi-temporal close.
//!
//! This module owns the *policy* (which predicates are singletons, what
//! supersession means); all reads and writes on the `triple` table go through
//! [`TripleStoreClient`], the single owner of that table.

use crate::service::error::MemoryError;
use crate::service::triple_extractor::SemanticTriple;
use crate::storage::TripleStoreClient;

/// Resolve conflicts for a newly created triple.
///
/// If the triple's predicate is a singleton, find any existing active triples
/// with the same (subject, predicate) but a different object and invalidate
/// them. Returns the ids of the invalidated triples.
pub(crate) async fn resolve_conflicts_for_triple(
    triple_store: &TripleStoreClient,
    new_triple: &SemanticTriple,
) -> Result<Vec<String>, MemoryError> {
    if !crate::service::triple_extractor::is_singleton_predicate(&new_triple.predicate) {
        return Ok(vec![]);
    }

    let conflicting = triple_store
        .find_conflicting_triple_ids(
            &new_triple.subject,
            &new_triple.predicate,
            &new_triple.object,
        )
        .await?;

    let mut invalidated = Vec::with_capacity(conflicting.len());
    for triple_id in &conflicting {
        triple_store.close_triple(triple_id).await?;
        invalidated.push(triple_id.clone());
    }

    Ok(invalidated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::DbClient;
    use std::sync::Arc;

    fn triple(subject: &str, predicate: &str, object: &str) -> SemanticTriple {
        SemanticTriple {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            confidence: 0.9,
            source_fact_id: "fact:new".to_string(),
        }
    }

    async fn embedded_triple_store() -> TripleStoreClient {
        let db_name = format!(
            "conflict_resolver_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        );
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                &db_name,
                &["org".to_string()],
                "error",
            )
            .await
            .expect("connect in memory db"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");
        TripleStoreClient::new(db_client, "org")
    }

    #[tokio::test]
    async fn resolve_conflicts_closes_superseded_triples() {
        let store = embedded_triple_store().await;
        let superseded = store
            .create_triple("alice", "works_at", "acme", 0.9, "fact:old")
            .await
            .expect("seed triple");

        let invalidated =
            resolve_conflicts_for_triple(&store, &triple("alice", "works_at", "globex"))
                .await
                .expect("resolve conflicts");

        assert_eq!(invalidated, vec![superseded]);

        let still_conflicting = store
            .find_conflicting_triple_ids("alice", "works_at", "globex")
            .await
            .expect("find conflicting");
        assert!(
            still_conflicting.is_empty(),
            "superseded triples must no longer appear as conflicts: {still_conflicting:?}"
        );
    }

    #[tokio::test]
    async fn resolve_conflicts_skips_non_singleton_predicates() {
        let store = embedded_triple_store().await;
        store
            .create_triple("alice", "knows", "bob", 0.9, "fact:old")
            .await
            .expect("seed triple");

        let invalidated = resolve_conflicts_for_triple(&store, &triple("alice", "knows", "carol"))
            .await
            .expect("resolve conflicts");

        assert!(
            invalidated.is_empty(),
            "non-singleton predicates must never supersede"
        );
    }

    #[tokio::test]
    async fn resolve_conflicts_keeps_triples_with_same_object() {
        let store = embedded_triple_store().await;
        let same_object = store
            .create_triple("alice", "works_at", "acme", 0.9, "fact:old")
            .await
            .expect("seed triple");

        let invalidated =
            resolve_conflicts_for_triple(&store, &triple("alice", "works_at", "acme"))
                .await
                .expect("resolve conflicts");

        assert!(invalidated.is_empty(), "same object is not a conflict");

        let active = store
            .find_conflicting_triple_ids("alice", "works_at", "globex")
            .await
            .expect("find conflicting");
        assert_eq!(active, vec![same_object]);
    }
}
