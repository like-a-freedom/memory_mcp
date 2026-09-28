//! Community summary construction for the memory-owned community table.
//!
//! Moved out of `service/episode/communities.rs` so the memory context's
//! community rebuild pass can call it without reaching into `service`.
//! The dependency struct it used to take is narrowed to the two fields it
//! actually reads, which is what removed the container requirement.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::storage::value_helpers::unwrap_string;

/// Build a human-readable summary of community members.
pub(crate) async fn build_community_summary(
    db_client: &Arc<dyn crate::storage::DbClient>,
    active_namespace: &str,
    member_entities: &[String],
) -> Result<String, MemoryError> {
    let records = crate::knowledge::graph_store::KnowledgeGraphStore::new(
        db_client.clone(),
        active_namespace.to_string(),
    )
    .select_entities_by_ids(member_entities)
    .await?;
    let mut names = records
        .iter()
        .filter_map(|record| record.as_object())
        .filter_map(|record| {
            record
                .get("canonical_name")
                .and_then(unwrap_string)
                .or_else(|| record.get("entity_id").and_then(unwrap_string))
                .or_else(|| record.get("id").and_then(unwrap_string))
        })
        .collect::<Vec<_>>();

    names.sort();
    names.dedup();

    let labels = if names.is_empty() {
        let mut fallback = member_entities.to_vec();
        fallback.sort();
        fallback.dedup();
        fallback
    } else {
        names
    };

    Ok(condense_community_labels(&labels))
}

fn condense_community_labels(labels: &[String]) -> String {
    let preview = labels
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let remaining = labels.len().saturating_sub(3);
    if remaining > 0 {
        format!("{preview} (+{remaining} more)")
    } else {
        preview
    }
}
