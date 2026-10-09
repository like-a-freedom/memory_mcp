use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::error::MemoryError;
use crate::models::Fact;

use super::filtering::filter_facts_by_constraints;
use super::query_mode::query_phrase_candidates;
use super::types::GraphAnchorOrigin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GraphTrace {
    pub(crate) anchor_entity_id: String,
    pub(crate) anchor_origin: GraphAnchorOrigin,
    pub(crate) anchor_canonical_name: String,
    pub(crate) hop_count: usize,
    pub(crate) path: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct GraphCandidate {
    pub(crate) fact: Fact,
    pub(crate) rationale: String,
    pub(crate) origin_factor: f64,
    pub(crate) trace: GraphTrace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphAnchor {
    canonical_name: String,
    origin: GraphAnchorOrigin,
}

const MAX_ADMITTED_GRAPH_SEEDS: usize = 32;

pub(super) fn admitted_graph_seed_facts(
    query: &super::admission::QueryEvidence,
    lexical_facts: &[Fact],
) -> Vec<Fact> {
    lexical_facts
        .iter()
        .filter(|fact| super::admission::episode_candidate_is_admitted(query, &fact.content))
        .take(MAX_ADMITTED_GRAPH_SEEDS)
        .cloned()
        .collect()
}

pub(crate) struct CollectGraphFactsRequest<'a> {
    pub(crate) cutoff_iso: &'a str,
    pub(crate) raw_query: &'a str,
    pub(crate) access: &'a crate::models::AccessPayload,
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
        .and_then(crate::storage::value_helpers::unwrap_record_string)
        .or_else(|| {
            map.get("id")
                .and_then(crate::storage::value_helpers::unwrap_record_string)
        })?;
    let canonical_name = map
        .get("canonical_name")
        .and_then(crate::storage::value_helpers::unwrap_record_string)
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
    service: &crate::memory::retrieval::RetrievalContext,
    raw_query: &str,
    lexical_facts: &[Fact],
) -> Result<BTreeMap<String, GraphAnchor>, MemoryError> {
    let normalized_names = query_phrase_candidates(raw_query)
        .into_iter()
        .map(|phrase| crate::shared::search::normalize_text(&phrase))
        .filter(|phrase| !phrase.is_empty())
        .collect::<Vec<_>>();

    let mut anchors = service
        .knowledge_store()
        .select_entities_batch(&normalized_names)
        .await?
        .into_iter()
        .filter_map(|value| entity_anchor_from_value(&value))
        .map(|(entity_id, canonical_name)| {
            (
                entity_id.clone(),
                GraphAnchor {
                    canonical_name,
                    origin: GraphAnchorOrigin::QueryEntity { entity_id },
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    for fact in lexical_facts.iter().take(MAX_ADMITTED_GRAPH_SEEDS) {
        for entity_id in &fact.entity_links {
            anchors
                .entry(entity_id.clone())
                .or_insert_with(|| GraphAnchor {
                    canonical_name: entity_id.clone(),
                    origin: GraphAnchorOrigin::AdmittedSeed {
                        fact_id: fact.fact_id.clone(),
                    },
                });
        }
    }

    Ok(anchors)
}

const GRAPH_NEIGHBOR_BATCH_SIZE: usize = 32;

fn record_graph_neighbor(
    frontier: &BTreeMap<String, GraphTrace>,
    traces: &mut HashMap<String, GraphTrace>,
    next_frontier: &mut BTreeMap<String, GraphTrace>,
    current_entity: &str,
    neighbor: &str,
) {
    if !neighbor.starts_with("entity:") {
        return;
    }

    let Some(current_trace) = frontier.get(current_entity) else {
        return;
    };
    let mut path = current_trace.path.clone();
    path.push(neighbor.to_string());
    let next_trace = GraphTrace {
        anchor_entity_id: current_trace.anchor_entity_id.clone(),
        anchor_origin: current_trace.anchor_origin.clone(),
        anchor_canonical_name: current_trace.anchor_canonical_name.clone(),
        hop_count: current_trace.hop_count + 1,
        path,
    };

    if insert_shortest_hop(traces, neighbor, next_trace.clone()) {
        next_frontier.insert(neighbor.to_string(), next_trace);
    }
}

async fn walk_anchor_entities(
    service: &crate::memory::retrieval::RetrievalContext,
    cutoff_iso: &str,
    anchors: &BTreeMap<String, GraphAnchor>,
    max_hops: usize,
) -> Result<HashMap<String, GraphTrace>, MemoryError> {
    let mut traces = HashMap::<String, GraphTrace>::new();
    let mut frontier = BTreeMap::<String, GraphTrace>::new();

    for (entity_id, anchor) in anchors {
        let trace = GraphTrace {
            anchor_entity_id: entity_id.clone(),
            anchor_origin: anchor.origin.clone(),
            anchor_canonical_name: anchor.canonical_name.clone(),
            hop_count: 0,
            path: vec![entity_id.clone()],
        };
        insert_shortest_hop(&mut traces, entity_id, trace.clone());
        frontier.insert(entity_id.clone(), trace);
    }

    while frontier
        .values()
        .next()
        .is_some_and(|trace| trace.hop_count < max_hops)
    {
        let frontier_ids = frontier.keys().cloned().collect::<Vec<_>>();
        let mut edges = Vec::new();
        for entity_batch in frontier_ids.chunks(GRAPH_NEIGHBOR_BATCH_SIZE) {
            edges.extend(
                service
                    .knowledge_store()
                    .select_edge_neighbors_for_entities(entity_batch, cutoff_iso)
                    .await?,
            );
        }

        let mut next_frontier = BTreeMap::new();
        for edge in edges {
            let Some(map) = edge.as_object() else {
                continue;
            };
            let in_id = map
                .get("in")
                .and_then(crate::storage::value_helpers::unwrap_record_string);
            let out_id = map
                .get("out")
                .and_then(crate::storage::value_helpers::unwrap_record_string);
            let (Some(in_id), Some(out_id)) = (in_id.as_deref(), out_id.as_deref()) else {
                continue;
            };

            record_graph_neighbor(&frontier, &mut traces, &mut next_frontier, in_id, out_id);
            record_graph_neighbor(&frontier, &mut traces, &mut next_frontier, out_id, in_id);
        }
        frontier = next_frontier;
    }

    Ok(traces)
}

pub(crate) async fn collect_graph_facts(
    service: &crate::memory::retrieval::RetrievalContext,
    request: CollectGraphFactsRequest<'_>,
) -> Result<Vec<GraphCandidate>, MemoryError> {
    if request.raw_query.trim().is_empty() || request.max_hops == 0 {
        return Ok(Vec::new());
    }

    let anchors =
        resolve_query_anchor_entities(service, request.raw_query, request.lexical_facts).await?;
    if anchors.is_empty() {
        return Ok(Vec::new());
    }

    let traces =
        walk_anchor_entities(service, request.cutoff_iso, &anchors, request.max_hops).await?;
    let entity_ids = traces.keys().cloned().collect::<Vec<_>>();
    let records = service
        .knowledge_store()
        .select_facts_by_entity_links(request.cutoff_iso, &entity_ids, request.budget.max(1) * 4)
        .await?;

    let mut facts = filter_facts_by_constraints(records, request.access, request.fact_types)
        .into_iter()
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
    use crate::models::EdgeAttributes;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};

    use super::*;
    use crate::error::MemoryError;
    use crate::memory::retrieval::query_mode::QueryFlags;
    use crate::models::{Fact, Provenance};
    use crate::storage::{DbClient, GraphDirection, SurrealDbClient};

    struct CountingGraphDb {
        inner: Arc<SurrealDbClient>,
        edge_query_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DbClient for CountingGraphDb {
        async fn select_one(
            &self,
            record_id: &str,
            namespace: &str,
        ) -> Result<Option<Value>, MemoryError> {
            self.inner.select_one(record_id, namespace).await
        }

        async fn select_table(
            &self,
            table: crate::storage::table_scope::OwnedTable,
            namespace: &str,
        ) -> Result<Vec<Value>, MemoryError> {
            self.inner.select_table(table, namespace).await
        }

        #[allow(clippy::too_many_arguments)]
        async fn create(
            &self,
            record_id: &str,
            content: Value,
            namespace: &str,
            temporal_fields: &[&str],
        ) -> Result<Value, MemoryError> {
            self.inner
                .create(record_id, content, namespace, temporal_fields)
                .await
        }

        async fn update(
            &self,
            record_id: &str,
            content: Value,
            namespace: &str,
            temporal_fields: &[&str],
        ) -> Result<Value, MemoryError> {
            self.inner
                .update(record_id, content, namespace, temporal_fields)
                .await
        }

        async fn query(
            &self,
            sql: &str,
            vars: Option<Value>,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            if sql.contains("FROM edge") {
                self.edge_query_count.fetch_add(1, Ordering::Relaxed);
            }
            self.inner.query(sql, vars, namespace).await
        }

        async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
            self.inner.apply_migrations(namespace).await
        }
    }

    #[tokio::test]
    async fn graph_neighbor_reads_batch_seed_nodes_into_one_store_call() {
        let namespaces = vec!["org".to_string()];
        let db = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "graph-batch-seed-read-test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in-memory graph database"),
        );
        db.apply_migrations("org")
            .await
            .expect("apply production migrations");
        let setup_service = crate::service::MemoryService::new(
            db.clone(),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("setup service");

        for (entity_id, name) in [
            ("entity:seedone", "Seed One"),
            ("entity:seedtwo", "Seed Two"),
            ("entity:seedthree", "Seed Three"),
            ("entity:target", "Target"),
        ] {
            db.create(
                entity_id,
                json!({
                    "entity_id": entity_id,
                    "entity_type": "person",
                    "canonical_name": name,
                    "canonical_name_normalized": crate::shared::search::normalize_text(name),
                    "aliases": [],
                }),
                "org",
                crate::knowledge::queries::ENTITY_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed graph entity");
        }
        for entity_id in ["entity:seedone", "entity:seedtwo", "entity:seedthree"] {
            setup_service
                .relate(
                    entity_id,
                    "knows",
                    "entity:target",
                    crate::models::EdgeAttributes::inferred(),
                )
                .await
                .expect("seed graph edge");
        }
        let cutoff_iso =
            crate::shared::temporal::normalize_dt(Utc::now() + chrono::Duration::seconds(1));
        let setup_retrieval =
            crate::memory::retrieval_deps::AssembleContextDeps::from(&setup_service);
        let outgoing = setup_retrieval
            .knowledge_store()
            .select_edge_neighbors("entity:seedone", &cutoff_iso, GraphDirection::Outgoing)
            .await
            .expect("read seeded graph edge");
        assert_eq!(
            outgoing.len(),
            1,
            "the query-count test must seed one visible edge: {outgoing:?}"
        );

        let edge_query_count = Arc::new(AtomicUsize::new(0));
        let recording_db = Arc::new(CountingGraphDb {
            inner: db,
            edge_query_count: edge_query_count.clone(),
        });
        let service = crate::service::MemoryService::new(
            recording_db,
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("retrieval service with query counter");
        let retrieval = crate::memory::retrieval_deps::AssembleContextDeps::from(&service);
        let anchors = ["entity:seedone", "entity:seedtwo", "entity:seedthree"]
            .into_iter()
            .map(|entity_id| {
                (
                    entity_id.to_string(),
                    GraphAnchor {
                        canonical_name: entity_id.to_string(),
                        origin: GraphAnchorOrigin::QueryEntity {
                            entity_id: entity_id.to_string(),
                        },
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        let traces = walk_anchor_entities(&retrieval, &cutoff_iso, &anchors, 1)
            .await
            .expect("walk one graph level");

        assert_eq!(
            traces.get("entity:target").map(|trace| trace.hop_count),
            Some(1)
        );
        assert_eq!(
            edge_query_count.load(Ordering::Relaxed),
            1,
            "three anchor nodes must be fetched as one batch, not two reads per node"
        );
    }

    #[test]
    fn insert_shortest_hop_keeps_the_smallest_depth_for_each_entity() {
        let mut traces = HashMap::new();

        assert!(insert_shortest_hop(
            &mut traces,
            "entity:bob",
            GraphTrace {
                anchor_entity_id: "entity:alice".to_string(),
                anchor_origin: GraphAnchorOrigin::QueryEntity {
                    entity_id: "entity:alice".to_string()
                },
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
                anchor_origin: GraphAnchorOrigin::QueryEntity {
                    entity_id: "entity:alice".to_string()
                },
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
                anchor_origin: GraphAnchorOrigin::QueryEntity {
                    entity_id: "entity:alice".to_string()
                },
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
        let namespaces = vec!["org".to_string()];
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
            "org".to_string(),
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
                    "canonical_name_normalized": crate::shared::search::normalize_text("Alice Stone"),
                    "aliases": [],
                }),
                "org",
                crate::knowledge::queries::ENTITY_TEMPORAL_FIELDS,
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
                    "canonical_name_normalized": crate::shared::search::normalize_text("Bob Chen"),
                    "aliases": [],
                }),
                "org",
                crate::knowledge::queries::ENTITY_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed bob");
        db_client
            .create(
                "entity:carol",
                json!({
                    "entity_id": "entity:carol",
                    "entity_type": "person",
                    "canonical_name": "Carol Diaz",
                    "canonical_name_normalized": crate::shared::search::normalize_text("Carol Diaz"),
                    "aliases": [],
                }),
                "org",
                crate::knowledge::queries::ENTITY_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed carol");
        db_client
            .create(
                "entity:dave",
                json!({
                    "entity_id": "entity:dave",
                    "entity_type": "person",
                    "canonical_name": "Dave Moore",
                    "canonical_name_normalized": crate::shared::search::normalize_text("Dave Moore"),
                    "aliases": [],
                }),
                "org",
                crate::knowledge::queries::ENTITY_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed dave");
        service
            .relate(
                "entity:alice",
                "knows",
                "entity:bob",
                EdgeAttributes::inferred(),
            )
            .await
            .expect("seed edge");

        service
            .relate(
                "entity:bob",
                "knows",
                "entity:carol",
                EdgeAttributes::inferred(),
            )
            .await
            .expect("seed second-hop edge");
        service
            .relate(
                "entity:carol",
                "knows",
                "entity:dave",
                EdgeAttributes::inferred(),
            )
            .await
            .expect("seed third-hop edge");

        let bob_fact_id = service
            .add_fact(
                "note",
                "Bob Chen owns the Atlas launch checklist.",
                "Bob Chen owns the Atlas launch checklist.",
                "episode:bob",
                t,
                0.9,
                vec!["entity:bob".to_string()],
                vec![],
                Provenance::agent_observation("episode:bob"),
            )
            .await
            .expect("seed Bob fact");
        let carol_fact_id = service
            .add_fact(
                "note",
                "Carol Diaz owns the Atlas launch migration plan.",
                "Carol Diaz owns the Atlas launch migration plan.",
                "episode:carol",
                t,
                0.9,
                vec!["entity:carol".to_string()],
                vec![],
                Provenance::agent_observation("episode:carol"),
            )
            .await
            .expect("seed Carol fact");
        let dave_fact_id = service
            .add_fact(
                "note",
                "Dave Moore owns the Atlas launch archive.",
                "Dave Moore owns the Atlas launch archive.",
                "episode:dave",
                t,
                0.9,
                vec!["entity:dave".to_string()],
                vec![],
                Provenance::agent_observation("episode:dave"),
            )
            .await
            .expect("seed Dave fact");

        let access = crate::models::AccessPayload {
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
        };
        let cutoff_iso = crate::shared::temporal::normalize_dt(cutoff);

        let retrieval = crate::memory::retrieval_deps::AssembleContextDeps::from(&service);
        let anchors = resolve_query_anchor_entities(&retrieval, "Alice Stone", &[])
            .await
            .expect("resolve anchors");
        assert_eq!(
            anchors
                .get("entity:alice")
                .map(|anchor| anchor.canonical_name.as_str()),
            Some("Alice Stone"),
            "expected Alice to be resolved as a graph anchor: {anchors:?}"
        );

        let outgoing_neighbors = retrieval
            .knowledge_store()
            .select_edge_neighbors("entity:alice", &cutoff_iso, GraphDirection::Outgoing)
            .await
            .expect("select outgoing neighbors");
        assert_eq!(
            outgoing_neighbors.len(),
            1,
            "expected Alice to have one outgoing neighbor edge: {outgoing_neighbors:?}"
        );

        let traces = walk_anchor_entities(&retrieval, &cutoff_iso, &anchors, 1)
            .await
            .expect("walk anchors");
        assert_eq!(
            traces.get("entity:bob").map(|trace| trace.hop_count),
            Some(1),
            "expected Bob to be discovered one hop away: {traces:?}"
        );

        let raw_records = retrieval
            .knowledge_store()
            .select_facts_by_entity_links(
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
            &retrieval,
            CollectGraphFactsRequest {
                cutoff_iso: &cutoff_iso,
                raw_query: "Alice Stone",
                access: &access,
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
        assert_eq!(
            candidates[0].trace.anchor_origin,
            GraphAnchorOrigin::QueryEntity {
                entity_id: "entity:alice".to_string()
            }
        );
        assert_eq!(candidates[0].trace.hop_count, 1);

        let seed_fact_id = service
            .add_fact(
                "note",
                "Atlas launch architecture depends on Alice.",
                "Atlas launch architecture depends on Alice.",
                "episode:alice-seed",
                t,
                0.9,
                vec!["entity:alice".to_string()],
                vec![],
                Provenance::agent_observation("episode:alice-seed"),
            )
            .await
            .expect("seed lexical graph anchor fact");
        let seed_fact = Fact {
            fact_id: seed_fact_id.clone(),
            fact_type: "note".to_string(),
            content: "Atlas launch architecture depends on Alice.".to_string(),
            quote: "Atlas launch architecture depends on Alice.".to_string(),
            source_episode: "episode:alice-seed".to_string(),
            t_valid: t,
            t_ingested: t,
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 0.9,
            index_keys: Vec::new(),
            access_count: 0,
            last_accessed: None,
            entity_links: vec!["entity:alice".to_string()],
            scope: "org".to_string(),
            policy_tags: Vec::new(),
            provenance: Provenance::agent_observation("episode:alice-seed"),
            ft_score: 0.0,
        };
        let seed_query = super::super::admission::build_query_evidence(
            Some("Atlas launch"),
            Vec::new(),
            &QueryFlags::default(),
        );
        let admitted_seeds = admitted_graph_seed_facts(&seed_query, &[seed_fact]);
        assert_eq!(admitted_seeds.len(), 1);

        let direct_fact_ids = HashSet::from([seed_fact_id.clone()]);
        let seeded_candidates = collect_graph_facts(
            &retrieval,
            CollectGraphFactsRequest {
                cutoff_iso: &cutoff_iso,
                raw_query: "Atlas launch",
                access: &access,
                fact_types: &[],
                direct_fact_ids: &direct_fact_ids,
                lexical_facts: &admitted_seeds,
                max_hops: 2,
                budget: 10,
            },
        )
        .await
        .expect("collect facts from the admitted seed anchor");
        let candidate_ids = seeded_candidates
            .iter()
            .map(|candidate| candidate.fact.fact_id.as_str())
            .collect::<std::collections::HashSet<_>>();

        assert!(candidate_ids.contains(bob_fact_id.as_str()));
        assert!(candidate_ids.contains(carol_fact_id.as_str()));
        assert!(!candidate_ids.contains(dave_fact_id.as_str()));
        assert!(seeded_candidates.iter().all(|candidate| {
            candidate.trace.anchor_origin
                == GraphAnchorOrigin::AdmittedSeed {
                    fact_id: seed_fact_id.clone(),
                }
        }));
    }
}
