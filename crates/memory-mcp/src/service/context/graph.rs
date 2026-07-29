use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::models::Fact;
use crate::service::error::MemoryError;
use crate::storage::GraphDirection;

use super::filtering::filter_facts_by_constraints;
use super::query_mode::query_phrase_candidates;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GraphTrace {
    pub(crate) anchor_entity_id: String,
    pub(crate) anchor_canonical_name: String,
    pub(crate) hop_count: usize,
    pub(crate) path: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct GraphCandidate {
    pub(crate) fact: Fact,
    pub(crate) rationale: String,
    pub(crate) origin_factor: f64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) trace: GraphTrace,
}

pub(crate) struct CollectGraphFactsRequest<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff_iso: &'a str,
    pub(crate) raw_query: &'a str,
    pub(crate) access: &'a crate::models::AccessPayload,
    pub(crate) project: Option<&'a str>,
    pub(crate) fact_types: &'a [String],
    pub(crate) direct_fact_ids: &'a HashSet<String>,
    pub(crate) lexical_facts: &'a [Fact],
    pub(crate) max_hops: usize,
    pub(crate) budget: i32,
}

fn entity_anchor_from_value(value: &Value) -> Option<(String, String)> {
    let map = value.as_object()?;
    let entity_id = map
        .get("entity_id")
        .and_then(crate::service::episode::unwrap_record_string)
        .or_else(|| {
            map.get("id")
                .and_then(crate::service::episode::unwrap_record_string)
        })?;
    let canonical_name = map
        .get("canonical_name")
        .and_then(crate::service::episode::unwrap_record_string)
        .unwrap_or_else(|| entity_id.clone());
    Some((entity_id, canonical_name))
}

fn insert_shortest_hop(
    traces: &mut HashMap<String, GraphTrace>,
    entity_id: &str,
    trace: GraphTrace,
) -> bool {
    match traces.get(entity_id) {
        Some(existing) if existing.hop_count <= trace.hop_count => false,
        _ => {
            traces.insert(entity_id.to_string(), trace);
            true
        }
    }
}

async fn resolve_query_anchor_entities(
    service: &crate::service::service_context::ServiceContext,
    namespace: &str,
    raw_query: &str,
    lexical_facts: &[Fact],
) -> Result<BTreeMap<String, String>, MemoryError> {
    let normalized_names = query_phrase_candidates(raw_query)
        .into_iter()
        .map(|phrase| crate::service::normalize_text(&phrase))
        .filter(|phrase| !phrase.is_empty())
        .collect::<Vec<_>>();

    let mut anchors = service
        .context_store()
        .select_entities_batch(namespace, &normalized_names)
        .await?
        .into_iter()
        .filter_map(|value| entity_anchor_from_value(&value))
        .collect::<BTreeMap<_, _>>();

    for fact in lexical_facts {
        for entity_id in &fact.entity_links {
            anchors
                .entry(entity_id.clone())
                .or_insert_with(|| entity_id.clone());
        }
    }

    Ok(anchors)
}

async fn walk_anchor_entities(
    service: &crate::service::service_context::ServiceContext,
    namespace: &str,
    cutoff_iso: &str,
    anchors: &BTreeMap<String, String>,
    max_hops: usize,
) -> Result<HashMap<String, GraphTrace>, MemoryError> {
    let mut traces = HashMap::<String, GraphTrace>::new();
    let mut queue = VecDeque::new();

    for (entity_id, canonical_name) in anchors {
        let trace = GraphTrace {
            anchor_entity_id: entity_id.clone(),
            anchor_canonical_name: canonical_name.clone(),
            hop_count: 0,
            path: vec![entity_id.clone()],
        };
        insert_shortest_hop(&mut traces, entity_id, trace.clone());
        queue.push_back((entity_id.clone(), trace));
    }

    while let Some((current_entity, current_trace)) = queue.pop_front() {
        if current_trace.hop_count >= max_hops {
            continue;
        }

        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            for edge in service
                .context_store()
                .select_edge_neighbors(namespace, &current_entity, cutoff_iso, direction)
                .await?
            {
                let Some(map) = edge.as_object() else {
                    continue;
                };
                let in_id = map
                    .get("in")
                    .and_then(crate::service::episode::unwrap_record_string);
                let out_id = map
                    .get("out")
                    .and_then(crate::service::episode::unwrap_record_string);
                let neighbor = match (in_id.as_deref(), out_id.as_deref()) {
                    (Some(left), Some(right)) if left == current_entity => Some(right.to_string()),
                    (Some(left), Some(right)) if right == current_entity => Some(left.to_string()),
                    _ => None,
                };
                let Some(neighbor) = neighbor else {
                    continue;
                };
                if !neighbor.starts_with("entity:") {
                    continue;
                }

                let mut next_path = current_trace.path.clone();
                next_path.push(neighbor.clone());
                let next_trace = GraphTrace {
                    anchor_entity_id: current_trace.anchor_entity_id.clone(),
                    anchor_canonical_name: current_trace.anchor_canonical_name.clone(),
                    hop_count: current_trace.hop_count + 1,
                    path: next_path,
                };

                if insert_shortest_hop(&mut traces, &neighbor, next_trace.clone()) {
                    queue.push_back((neighbor, next_trace));
                }
            }
        }
    }

    Ok(traces)
}

pub(crate) async fn collect_graph_facts(
    service: &crate::service::service_context::ServiceContext,
    request: CollectGraphFactsRequest<'_>,
) -> Result<Vec<GraphCandidate>, MemoryError> {
    if request.raw_query.trim().is_empty() || request.max_hops == 0 {
        return Ok(Vec::new());
    }

    let anchors = resolve_query_anchor_entities(
        service,
        request.namespace,
        request.raw_query,
        request.lexical_facts,
    )
    .await?;
    if anchors.is_empty() {
        return Ok(Vec::new());
    }

    let traces = walk_anchor_entities(
        service,
        request.namespace,
        request.cutoff_iso,
        &anchors,
        request.max_hops,
    )
    .await?;
    let entity_ids = traces.keys().cloned().collect::<Vec<_>>();
    let records = service
        .context_store()
        .select_facts_by_entity_links(
            request.namespace,
            request.scope,
            request.cutoff_iso,
            &entity_ids,
            request.budget.max(1) * 4,
        )
        .await?;

    let mut facts =
        filter_facts_by_constraints(records, request.access, request.project, request.fact_types)
            .into_iter()
            .filter(|fact| fact.scope == request.scope)
            .filter(|fact| !request.direct_fact_ids.contains(&fact.fact_id))
            .collect::<Vec<_>>();
    facts.sort_by(|left, right| {
        right
            .t_valid
            .cmp(&left.t_valid)
            .then_with(|| left.fact_id.cmp(&right.fact_id))
    });

    Ok(facts
        .into_iter()
        .filter_map(|fact| {
            let trace = fact
                .entity_links
                .iter()
                .filter_map(|entity_id| traces.get(entity_id))
                .min_by(|left, right| {
                    left.hop_count
                        .cmp(&right.hop_count)
                        .then_with(|| left.anchor_entity_id.cmp(&right.anchor_entity_id))
                })?
                .clone();

            Some(GraphCandidate {
                rationale: format!(
                    "matched graph anchor={} hops={} path={}",
                    trace.anchor_canonical_name,
                    trace.hop_count,
                    trace.path.join(" -> ")
                ),
                origin_factor: 1.0,
                trace,
                fact,
            })
        })
        .take(request.budget.max(1) as usize)
        .collect())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::models::Provenance;
    use crate::storage::DbClient;

    #[test]
    fn insert_shortest_hop_keeps_the_smallest_depth_for_each_entity() {
        let mut traces = HashMap::new();

        assert!(insert_shortest_hop(
            &mut traces,
            "entity:bob",
            GraphTrace {
                anchor_entity_id: "entity:alice".to_string(),
                anchor_canonical_name: "Alice Stone".to_string(),
                hop_count: 2,
                path: vec!["entity:alice".to_string(), "entity:bob".to_string()],
            },
        ));
        assert!(!insert_shortest_hop(
            &mut traces,
            "entity:bob",
            GraphTrace {
                anchor_entity_id: "entity:alice".to_string(),
                anchor_canonical_name: "Alice Stone".to_string(),
                hop_count: 3,
                path: vec![
                    "entity:alice".to_string(),
                    "episode:1".to_string(),
                    "entity:bob".to_string(),
                ],
            },
        ));
        assert!(insert_shortest_hop(
            &mut traces,
            "entity:bob",
            GraphTrace {
                anchor_entity_id: "entity:alice".to_string(),
                anchor_canonical_name: "Alice Stone".to_string(),
                hop_count: 1,
                path: vec!["entity:alice".to_string(), "entity:bob".to_string()],
            },
        ));

        assert_eq!(
            traces.get("entity:bob").map(|trace| trace.hop_count),
            Some(1)
        );
    }

    #[tokio::test]
    async fn collect_graph_facts_returns_one_hop_anchor_neighbor_fact() {
        let namespaces = vec![
            "org".to_string(),
            "personal".to_string(),
            "private".to_string(),
        ];
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                "graph-collector-test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        for namespace in &namespaces {
            db_client
                .apply_migrations(namespace)
                .await
                .expect("apply migrations");
        }

        let service = crate::service::MemoryService::new(
            db_client.clone(),
            namespaces,
            "warn".to_string(),
            50,
            100,
        )
        .expect("service init");

        let t = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();
        let cutoff = Utc::now() + chrono::Duration::seconds(1);
        db_client
            .create(
                "entity:alice",
                json!({
                    "entity_id": "entity:alice",
                    "entity_type": "person",
                    "canonical_name": "Alice Stone",
                    "canonical_name_normalized": crate::service::normalize_text("Alice Stone"),
                    "aliases": [],
                }),
                "org",
            )
            .await
            .expect("seed alice");
        db_client
            .create(
                "entity:bob",
                json!({
                    "entity_id": "entity:bob",
                    "entity_type": "person",
                    "canonical_name": "Bob Chen",
                    "canonical_name_normalized": crate::service::normalize_text("Bob Chen"),
                    "aliases": [],
                }),
                "org",
            )
            .await
            .expect("seed bob");
        service
            .relate("entity:alice", "knows", "entity:bob")
            .await
            .expect("seed edge");

        service
            .add_fact(
                "note",
                "Bob Chen owns the Atlas launch checklist.",
                "Bob Chen owns the Atlas launch checklist.",
                "episode:seed",
                t,
                "org",
                0.9,
                vec!["entity:bob".to_string()],
                vec![],
                Provenance::agent_observation("episode:seed"),
            )
            .await
            .expect("seed fact");

        let access = crate::models::AccessPayload {
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        let cutoff_iso = crate::service::normalize_dt(cutoff);

        let anchors =
            resolve_query_anchor_entities(&service.build_context(), "org", "Alice Stone", &[])
                .await
                .expect("resolve anchors");
        assert_eq!(
            anchors.get("entity:alice").map(String::as_str),
            Some("Alice Stone"),
            "expected Alice to be resolved as a graph anchor: {anchors:?}"
        );

        let ctx = service.build_context();
        let outgoing_neighbors = ctx
            .context_store()
            .select_edge_neighbors("org", "entity:alice", &cutoff_iso, GraphDirection::Outgoing)
            .await
            .expect("select outgoing neighbors");
        assert_eq!(
            outgoing_neighbors.len(),
            1,
            "expected Alice to have one outgoing neighbor edge: {outgoing_neighbors:?}"
        );

        let traces =
            walk_anchor_entities(&service.build_context(), "org", &cutoff_iso, &anchors, 1)
                .await
                .expect("walk anchors");
        assert_eq!(
            traces.get("entity:bob").map(|trace| trace.hop_count),
            Some(1),
            "expected Bob to be discovered one hop away: {traces:?}"
        );

        let raw_records = ctx
            .context_store()
            .select_facts_by_entity_links(
                "org",
                "org",
                &cutoff_iso,
                &traces.keys().cloned().collect::<Vec<_>>(),
                20,
            )
            .await
            .expect("select facts by entity links");
        assert_eq!(
            raw_records.len(),
            1,
            "expected one raw graph fact record: {raw_records:?}"
        );

        let candidates = collect_graph_facts(
            &service.build_context(),
            CollectGraphFactsRequest {
                namespace: "org",
                scope: "org",
                cutoff_iso: &cutoff_iso,
                raw_query: "Alice Stone",
                access: &access,
                project: None,
                fact_types: &[],
                direct_fact_ids: &HashSet::new(),
                lexical_facts: &[],
                max_hops: 1,
                budget: 5,
            },
        )
        .await
        .expect("collect graph facts");

        assert_eq!(
            candidates.len(),
            1,
            "expected one graph candidate: {candidates:?}"
        );
        assert!(
            candidates[0]
                .fact
                .content
                .contains("Atlas launch checklist")
        );
        assert_eq!(candidates[0].trace.anchor_entity_id, "entity:alice");
        assert_eq!(candidates[0].trace.hop_count, 1);
    }
}
