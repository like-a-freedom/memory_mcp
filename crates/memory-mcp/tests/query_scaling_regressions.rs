#![cfg(all(feature = "eval-support", feature = "test-fixtures"))]

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_mcp::knowledge::graph_store::KnowledgeGraphStore;
use memory_mcp::memory::fact_access_store::FactAccessStore;
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::storage::{DbClient, GraphDirection, SurrealDbClient};
use serde_json::{Value, json};

#[derive(Clone)]
struct QueryCountingDb {
    inner: Arc<SurrealDbClient>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl DbClient for QueryCountingDb {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, memory_mcp::MemoryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.select_one(record_id, namespace).await
    }

    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        namespace: &str,
    ) -> Result<Vec<Value>, memory_mcp::MemoryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.select_table(table, namespace).await
    }

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<Value, memory_mcp::MemoryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
    ) -> Result<Value, memory_mcp::MemoryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner
            .update(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, memory_mcp::MemoryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.query(sql, vars, namespace).await
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), memory_mcp::MemoryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.apply_migrations(namespace).await
    }
}

struct LexicalFallbackDb {
    inner: Arc<SurrealDbClient>,
    as_of_scans: Arc<AtomicUsize>,
    as_of_scan_fact_ids: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl DbClient for LexicalFallbackDb {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, memory_mcp::MemoryError> {
        self.inner.select_one(record_id, namespace).await
    }

    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        namespace: &str,
    ) -> Result<Vec<Value>, memory_mcp::MemoryError> {
        self.inner.select_table(table, namespace).await
    }

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<Value, memory_mcp::MemoryError> {
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
    ) -> Result<Value, memory_mcp::MemoryError> {
        self.inner
            .update(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, memory_mcp::MemoryError> {
        if sql.starts_with("RETURN [") {
            let group_count = sql.matches("(SELECT").count();
            return Ok(Value::Array(
                (0..group_count).map(|_| Value::Array(Vec::new())).collect(),
            ));
        }
        if sql.contains("search::score") {
            return Ok(Value::Array(Vec::new()));
        }
        let is_as_of_scan = sql.starts_with("SELECT id,") && sql.contains("FROM fact");
        if is_as_of_scan {
            assert!(sql.contains("t_ingested IS NONE"), "{sql}");
            assert!(
                sql.contains("t_invalid_ingested > type::datetime($cutoff)"),
                "{sql}"
            );
            self.as_of_scans.fetch_add(1, Ordering::SeqCst);
        }
        let result = self.inner.query(sql, vars, namespace).await?;
        if is_as_of_scan {
            let fact_ids = result
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row.get("fact_id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            *self
                .as_of_scan_fact_ids
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = fact_ids;
        }
        Ok(result)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), memory_mcp::MemoryError> {
        self.inner.apply_migrations(namespace).await
    }
}

fn record_id(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned).or_else(|| {
        let record = value.get("RecordId").unwrap_or(value);
        let table = record.get("table").or_else(|| record.get("tb"))?.as_str()?;
        let key = record.get("key").or_else(|| record.get("id"))?;
        match key {
            Value::String(id) => Some(format!("{table}:{id}")),
            Value::Number(id) => Some(format!("{table}:{id}")),
            _ => None,
        }
    })
}

fn logical_edge_id(edge: &Value) -> Option<String> {
    Some(format!(
        "{}:{}:{}",
        record_id(edge.get("in")?)?,
        edge.get("relation")?.as_str()?,
        record_id(edge.get("out")?)?
    ))
}

async fn seed_active_edge(db: &Arc<SurrealDbClient>, edge_id: &str, from_id: &str, to_id: &str) {
    KnowledgeGraphStore::new(db.clone(), "org")
        .relate_edge(
            edge_id,
            from_id,
            to_id,
            json!({
                "edge_id": edge_id,
                "in": from_id,
                "relation": "linked",
                "out": to_id,
                "origin": "inferred",
                "strength": 1.0,
                "confidence": 0.8,
                "provenance": {},
                "t_valid": "2026-01-01T00:00:00Z",
                "t_ingested": "2026-01-01T00:00:00Z",
            }),
        )
        .await
        .expect("seed active edge");
}

#[tokio::test]
async fn bounded_hub_candidates_preserve_the_existing_entity_prefix() {
    let memory = common::TestMemory::new(false).await;
    for index in 0..40 {
        let entity_id = format!("entity:prefix-{index:02}");
        let name = format!("Prefix {index:02}");
        common::seed_entity(&memory.db_client, "org", &entity_id, "person", &name, &[]).await;
    }
    let store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");
    let baseline = store.select_entities().await.expect("baseline entities");
    let prefix = baseline
        .iter()
        .take(17)
        .map(|row| row["entity_id"].clone())
        .collect::<Vec<_>>();

    let bounded = store
        .select_hub_candidates(17)
        .await
        .expect("bounded candidates");
    let bounded_prefix = bounded
        .iter()
        .map(|row| row["entity_id"].clone())
        .collect::<Vec<_>>();

    assert_eq!(bounded_prefix, prefix);
}

#[tokio::test]
async fn one_batched_hub_edge_read_matches_per_entity_direction_reads() {
    let memory = common::TestMemory::new(false).await;
    for index in 0..12 {
        let entity_id = format!("entity:hub-{index:02}");
        common::seed_entity(
            &memory.db_client,
            "org",
            &entity_id,
            "person",
            &entity_id,
            &[],
        )
        .await;
    }

    let cutoff = "2027-01-01T00:00:00Z";
    let candidate_store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");
    let prefix = candidate_store
        .select_entities()
        .await
        .expect("candidate entities")
        .into_iter()
        .take(12)
        .map(|entity| {
            record_id(&entity["entity_id"])
                .unwrap_or_else(|| panic!("candidate has unexpected id: {entity}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(prefix.len(), 12);
    let late_hub = prefix.last().expect("last candidate").clone();
    for (index, neighbor_id) in prefix.iter().take(10).enumerate() {
        seed_active_edge(
            &memory.db_client,
            &format!("edge:late-{index}"),
            &late_hub,
            neighbor_id,
        )
        .await;
    }
    seed_active_edge(
        &memory.db_client,
        "edge:late-duplicate",
        &late_hub,
        &prefix[0],
    )
    .await;
    seed_active_edge(&memory.db_client, "edge:late-self", &late_hub, &late_hub).await;

    let ids = prefix;
    let sequential_calls = Arc::new(AtomicUsize::new(0));
    let sequential_db = Arc::new(QueryCountingDb {
        inner: memory.db_client.clone(),
        calls: Arc::clone(&sequential_calls),
    });
    let sequential_store = KnowledgeGraphStore::new(sequential_db, "org");
    let mut sequential_edges = HashSet::new();
    for entity_id in &ids {
        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            for edge in sequential_store
                .select_edge_neighbors(entity_id, cutoff, direction)
                .await
                .expect("sequential neighbors")
            {
                sequential_edges
                    .insert(logical_edge_id(&edge).unwrap_or_else(|| {
                        panic!("sequential edge has unexpected fields: {edge}")
                    }));
            }
        }
    }
    assert_eq!(sequential_calls.load(Ordering::SeqCst), ids.len() * 2);

    let calls = Arc::new(AtomicUsize::new(0));
    let counted_db = Arc::new(QueryCountingDb {
        inner: memory.db_client.clone(),
        calls: Arc::clone(&calls),
    });
    let batch_store = KnowledgeGraphStore::new(counted_db, "org");
    let batch_edges = batch_store
        .select_hub_neighbor_edges(&ids, cutoff)
        .await
        .expect("batched neighbors");
    let batched_identities = batch_edges
        .iter()
        .map(|edge| {
            logical_edge_id(edge)
                .unwrap_or_else(|| panic!("batched edge has unexpected fields: {edge}"))
        })
        .collect::<HashSet<_>>();

    assert_eq!(batched_identities, sequential_edges);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let mut unique_degree = HashMap::<String, HashSet<String>>::new();
    for edge in &batch_edges {
        let identity = logical_edge_id(edge)
            .unwrap_or_else(|| panic!("batched edge has unexpected fields: {edge}"));
        for endpoint in [edge.get("in"), edge.get("out")].into_iter().flatten() {
            if let Some(entity_id) = record_id(endpoint)
                && ids.contains(&entity_id)
            {
                unique_degree
                    .entry(entity_id)
                    .or_default()
                    .insert(identity.clone());
            }
        }
    }
    assert_eq!(unique_degree[&late_hub].len(), 11);
}

#[tokio::test]
async fn map_view_preserves_hub_ranking_while_reducing_neighbor_reads() {
    let memory = common::TestMemory::new(false).await;
    for index in 0..12 {
        let entity_id = format!("entity:map-hub-{index:02}");
        common::seed_entity(
            &memory.db_client,
            "org",
            &entity_id,
            "person",
            &format!("Map hub {index:02}"),
            &[],
        )
        .await;
    }

    let cutoff = DateTime::parse_from_rfc3339("2027-01-01T00:00:00Z")
        .expect("cutoff")
        .with_timezone(&Utc);
    let cutoff_iso = memory_mcp::service::normalize_dt(cutoff);
    let baseline_store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");
    let candidate_rows = baseline_store
        .select_entities()
        .await
        .expect("baseline candidate entities");
    let candidates = candidate_rows
        .into_iter()
        .take(12 * 12)
        .filter_map(|row| {
            let entity_id = record_id(&row["entity_id"])?;
            let canonical_name = row["canonical_name"]
                .as_str()
                .unwrap_or(&entity_id)
                .to_owned();
            Some((entity_id, canonical_name))
        })
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 12);
    let late_hub = candidates.last().expect("late candidate").0.clone();
    for (index, (neighbor, _)) in candidates.iter().take(10).enumerate() {
        seed_active_edge(
            &memory.db_client,
            &format!("edge:map-late-{index}"),
            &late_hub,
            neighbor,
        )
        .await;
    }
    seed_active_edge(
        &memory.db_client,
        "edge:map-duplicate",
        &late_hub,
        &candidates[0].0,
    )
    .await;
    seed_active_edge(&memory.db_client, "edge:map-self", &late_hub, &late_hub).await;

    // Independent oracle for the pre-batching map-view implementation: read
    // each candidate in both directions and rank unique logical edge IDs.
    let mut expected_hubs = Vec::new();
    for (entity_id, canonical_name) in &candidates {
        let mut identities = HashSet::new();
        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            for edge in baseline_store
                .select_edge_neighbors(entity_id, &cutoff_iso, direction)
                .await
                .expect("baseline neighbors")
            {
                identities.insert(logical_edge_id(&edge).expect("baseline edge identity"));
            }
        }
        if !identities.is_empty() {
            expected_hubs.push((entity_id.clone(), canonical_name.clone(), identities.len()));
        }
    }
    expected_hubs.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.0.cmp(&right.0))
    });
    expected_hubs.truncate(12);

    let calls = Arc::new(AtomicUsize::new(0));
    let counting_db = Arc::new(QueryCountingDb {
        inner: memory.db_client.clone(),
        calls: Arc::clone(&calls),
    });
    let service = memory_mcp::service::MemoryService::new(
        counting_db,
        "org".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("counting map-view service");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: String::new(),
            as_of: Some(cutoff),
            budget: 12,
            fact_types: Vec::new(),
            view_mode: Some("map".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("assemble map view");
    let actual_hubs = items
        .iter()
        .filter(|item| item.provenance["kind"] == "hub_entity")
        .map(|item| {
            (
                item.provenance["entity_id"]
                    .as_str()
                    .expect("hub entity id")
                    .to_owned(),
                item.provenance["canonical_name"]
                    .as_str()
                    .expect("hub canonical name")
                    .to_owned(),
                item.provenance["degree"].as_u64().expect("hub degree") as usize,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(actual_hubs, expected_hubs);
    assert_eq!(
        actual_hubs.first().map(|hub| hub.0.as_str()),
        Some(late_hub.as_str())
    );
    assert!(
        calls.load(Ordering::SeqCst) < candidates.len() * 2,
        "map view must use fewer than {} old per-entity direction reads",
        candidates.len() * 2
    );
}

#[tokio::test]
async fn hub_edge_reads_chunk_candidate_ids_at_the_owner_limit() {
    let memory = common::TestMemory::new(false).await;
    let ids = (0..33)
        .map(|index| format!("entity:chunk-{index:02}"))
        .collect::<Vec<_>>();
    for entity_id in &ids {
        common::seed_entity(
            &memory.db_client,
            "org",
            entity_id,
            "person",
            entity_id,
            &[],
        )
        .await;
    }
    seed_active_edge(&memory.db_client, "edge:chunk-boundary", &ids[0], &ids[32]).await;

    let calls = Arc::new(AtomicUsize::new(0));
    let counted_db = Arc::new(QueryCountingDb {
        inner: memory.db_client.clone(),
        calls: Arc::clone(&calls),
    });
    let store = KnowledgeGraphStore::new(counted_db, "org");
    let edges = store
        .select_hub_neighbor_edges(&ids, "2027-01-01T00:00:00Z")
        .await
        .expect("chunked neighbors");

    let identities = edges
        .iter()
        .map(|edge| logical_edge_id(edge).expect("logical edge identity"))
        .collect::<HashSet<_>>();
    assert_eq!(identities.len(), 1);
    assert_eq!(
        identities.iter().next().map(String::as_str),
        Some("entity:chunk-00:linked:entity:chunk-32")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn batched_archival_signals_match_bitemporal_and_heat_checks() {
    let memory = common::TestMemory::new(false).await;
    let t_valid: DateTime<Utc> = "2026-01-01T00:00:00Z".parse().expect("valid fixture time");
    let cases = [
        ("active", "active signal fixture"),
        ("late-invalidation", "late invalidation fixture"),
        ("expired-invalidation", "expired invalidation fixture"),
        ("hot-only", "recent access fixture"),
        ("archiveable", "archiveable fixture"),
    ];
    let mut episode_by_case = HashMap::new();
    let mut fact_by_case = HashMap::new();
    for (source_id, content) in cases {
        let fact_id = common::seed_episode_backed_fact_with_source_id(
            &memory.service,
            "org",
            content,
            t_valid,
            source_id,
        )
        .await;
        let fact = memory
            .db_client
            .select_one(&fact_id, "org")
            .await
            .expect("read seeded fact")
            .expect("seeded fact exists");
        let episode_id = record_id(&fact["source_episode"]).expect("fact source episode");
        episode_by_case.insert(source_id, episode_id);
        fact_by_case.insert(source_id, fact_id);
    }

    for source_id in [
        "late-invalidation",
        "expired-invalidation",
        "hot-only",
        "archiveable",
    ] {
        let fact_id = &fact_by_case[source_id];
        let invalid_ingested = if source_id == "late-invalidation" {
            "2030-07-01T00:00:00Z"
        } else {
            "2026-03-01T00:00:00Z"
        };
        let last_accessed = if source_id == "hot-only" {
            memory_mcp::service::normalize_dt(Utc::now() - chrono::Duration::days(1))
        } else {
            "2026-01-01T00:00:00Z".to_string()
        };
        memory
            .db_client
            .query(
                "UPDATE type::record('fact', $fact_key) SET \
                 t_valid = type::datetime($t_valid), \
                 t_ingested = type::datetime($t_ingested), \
                 t_invalid = type::datetime($t_invalid), \
                 t_invalid_ingested = type::datetime($t_invalid_ingested), \
                 last_accessed = type::datetime($last_accessed)",
                Some(json!({
                    "fact_key": fact_id.strip_prefix("fact:").expect("fact record id"),
                    "t_valid": "2026-01-01T00:00:00Z",
                    "t_ingested": "2026-01-01T00:00:00Z",
                    "t_invalid": "2026-02-01T00:00:00Z",
                    "t_invalid_ingested": invalid_ingested,
                    "last_accessed": last_accessed,
                })),
                "org",
            )
            .await
            .expect("set archival temporal signals");
    }
    let active_fact_id = &fact_by_case["active"];
    memory
        .db_client
        .query(
            "UPDATE type::record('fact', $fact_key) SET \
             t_valid = type::datetime($t_valid), \
             t_ingested = type::datetime($t_ingested)",
            Some(json!({
                "fact_key": active_fact_id.strip_prefix("fact:").expect("fact record id"),
                "t_valid": "2026-01-01T00:00:00Z",
                "t_ingested": "2026-01-01T00:00:00Z",
            })),
            "org",
        )
        .await
        .expect("set active fact temporal signals");

    let cutoff = "2026-06-01T00:00:00Z";
    let hot_cutoff = "2026-06-15T00:00:00Z";
    let episode_ids = cases
        .iter()
        .map(|(source_id, _)| episode_by_case[source_id].clone())
        .collect::<Vec<_>>();
    let graph_store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");
    let access_store = FactAccessStore::new(memory.db_client.clone(), "org");
    let mut expected_active = HashSet::new();
    let mut expected_hot = HashSet::new();
    for episode_id in &episode_ids {
        if graph_store
            .has_active_facts_by_episode(episode_id, cutoff)
            .await
            .expect("per-episode active check")
        {
            expected_active.insert(episode_id.clone());
        }
        if access_store
            .has_recent_fact_access(episode_id, hot_cutoff)
            .await
            .expect("per-episode recent heat check")
        {
            expected_hot.insert(episode_id.clone());
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let counted_db = Arc::new(QueryCountingDb {
        inner: memory.db_client.clone(),
        calls: Arc::clone(&calls),
    });
    let batched_active = KnowledgeGraphStore::new(counted_db.clone(), "org")
        .episode_ids_with_active_facts(&episode_ids, cutoff)
        .await
        .expect("batched active facts")
        .into_iter()
        .collect::<HashSet<_>>();
    let batched_hot = FactAccessStore::new(counted_db, "org")
        .episode_ids_with_recent_fact_access(&episode_ids, hot_cutoff)
        .await
        .expect("batched access heat")
        .into_iter()
        .collect::<HashSet<_>>();

    assert_eq!(batched_active, expected_active);
    assert_eq!(batched_hot, expected_hot);
    assert_eq!(
        batched_active.len(),
        2,
        "active and late-ingested invalidation stay visible at cutoff"
    );
    assert_eq!(
        batched_hot,
        HashSet::from([episode_by_case["hot-only"].clone()])
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let archival_calls = Arc::new(AtomicUsize::new(0));
    let archival_db = Arc::new(QueryCountingDb {
        inner: memory.db_client.clone(),
        calls: Arc::clone(&archival_calls),
    });
    let archival_service = memory_mcp::service::MemoryService::new(
        archival_db,
        "org".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("counting archival service");
    archival_calls.store(0, Ordering::SeqCst);
    let archived = memory_mcp::platform::lifecycle_runtime::archival_pass(&archival_service, 90)
        .await
        .expect("archival pass");
    assert_eq!(archived, 2);
    assert_eq!(archival_calls.load(Ordering::SeqCst), 5);

    for source_id in ["active", "late-invalidation", "hot-only"] {
        let episode = memory
            .db_client
            .select_one(&episode_by_case[source_id], "org")
            .await
            .expect("read protected episode")
            .expect("protected episode exists");
        assert_ne!(episode["status"], "archived", "{source_id} must stay live");
    }
    for source_id in ["expired-invalidation", "archiveable"] {
        let episode = memory
            .db_client
            .select_one(&episode_by_case[source_id], "org")
            .await
            .expect("read archiveable episode")
            .expect("archiveable episode exists");
        assert_eq!(episode["status"], "archived", "{source_id} must archive");
    }
}

#[tokio::test]
async fn archival_signal_reads_chunk_episode_ids_at_sixty_four() {
    let memory = common::TestMemory::new(false).await;
    let episode_ids = (0..65)
        .map(|index| format!("episode:archive-chunk-{index:02}"))
        .collect::<Vec<_>>();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted_db = Arc::new(QueryCountingDb {
        inner: memory.db_client.clone(),
        calls: Arc::clone(&calls),
    });

    assert!(
        KnowledgeGraphStore::new(counted_db.clone(), "org")
            .episode_ids_with_active_facts(&episode_ids, "2026-06-01T00:00:00Z")
            .await
            .expect("chunked active signals")
            .is_empty()
    );
    assert!(
        FactAccessStore::new(counted_db, "org")
            .episode_ids_with_recent_fact_access(&episode_ids, "2026-06-01T00:00:00Z")
            .await
            .expect("chunked heat signals")
            .is_empty()
    );

    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn lexical_fallback_uses_full_bitemporal_visibility() {
    let memory = common::TestMemory::new(false).await;
    let valid_at = DateTime::parse_from_rfc3339("2025-12-01T00:00:00Z")
        .expect("valid timestamp")
        .with_timezone(&Utc);
    let future_ingested = common::seed_episode_backed_fact_with_source_id(
        &memory.service,
        "org",
        "Orion launch mission begins soon.",
        valid_at,
        "future-ingested",
    )
    .await;
    let late_invalidation = common::seed_episode_backed_fact_with_source_id(
        &memory.service,
        "org",
        "Orion launch mission remains valid until its invalidation is known.",
        valid_at,
        "late-invalidation",
    )
    .await;
    memory
        .db_client
        .update(
            &future_ingested,
            json!({"t_ingested": "2026-04-01T00:00:00Z"}),
            "org",
            &["t_ingested"],
        )
        .await
        .expect("set future ingestion time");
    memory
        .db_client
        .update(
            &late_invalidation,
            json!({
                "t_ingested": "2025-12-01T00:00:00Z",
                "t_invalid": "2026-01-10T00:00:00Z",
                "t_invalid_ingested": "2026-04-01T00:00:00Z",
            }),
            "org",
            &["t_ingested", "t_invalid", "t_invalid_ingested"],
        )
        .await
        .expect("set late invalidation");

    let seeded_rows = memory
        .db_client
        .query(
            "SELECT fact_id, t_valid, t_ingested, t_invalid, t_invalid_ingested FROM fact WHERE fact_id IN $fact_ids",
            Some(json!({"fact_ids": [future_ingested, late_invalidation]})),
            "org",
        )
        .await
        .expect("read temporal fixture state");
    assert_eq!(
        seeded_rows.as_array().map(Vec::len),
        Some(2),
        "fixture facts must exist before the visibility query: {seeded_rows}"
    );

    let as_of_scans = Arc::new(AtomicUsize::new(0));
    let as_of_scan_fact_ids = Arc::new(Mutex::new(Vec::new()));
    let service = memory_mcp::service::MemoryService::new(
        Arc::new(LexicalFallbackDb {
            inner: memory.db_client.clone(),
            as_of_scans: Arc::clone(&as_of_scans),
            as_of_scan_fact_ids: Arc::clone(&as_of_scan_fact_ids),
        }),
        "org".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service with lexical query observer");
    let cutoff = DateTime::parse_from_rfc3339("2026-01-15T00:00:00Z")
        .expect("cutoff")
        .with_timezone(&Utc);
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "Orion launch mission".to_string(),
            as_of: Some(cutoff),
            budget: 10,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("assemble lexical fallback context");
    let fact_ids = items
        .iter()
        .map(|item| item.fact_id.as_str())
        .collect::<HashSet<_>>();
    let scan_fact_ids = as_of_scan_fact_ids
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .cloned()
        .collect::<HashSet<_>>();

    assert!(
        scan_fact_ids.contains(&late_invalidation),
        "late invalidation should be visible at the historical cutoff; scanned={scan_fact_ids:?}, output={fact_ids:?}, rows={seeded_rows}, scans={}",
        as_of_scans.load(Ordering::SeqCst)
    );
    assert!(!scan_fact_ids.contains(&future_ingested));
    assert!(!fact_ids.contains(future_ingested.as_str()));
    assert_eq!(as_of_scans.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn duplicate_fact_accesses_each_increment_and_saturate_at_i64_max() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = common::seed_episode_backed_fact_with_source_id(
        &memory.service,
        "org",
        "Repeated fact access saturation fixture.",
        Utc::now(),
        "duplicate-access-saturation",
    )
    .await;
    memory
        .db_client
        .update(&fact_id, json!({"access_count": i64::MAX - 2}), "org", &[])
        .await
        .expect("set count close to upper bound");

    let store = FactAccessStore::new(memory.db_client.clone(), "org");
    store
        .record_fact_access(&fact_id, 1)
        .await
        .expect("first access");
    store
        .record_fact_access(&fact_id, 1)
        .await
        .expect("duplicate result access");
    let fact = memory
        .db_client
        .select_one(&fact_id, "org")
        .await
        .expect("read accessed fact")
        .expect("fact remains present");

    assert_eq!(fact["access_count"], i64::MAX);
}

#[tokio::test]
async fn community_offset_page_repeats_an_edge_after_an_earlier_insert() {
    let memory = common::TestMemory::new(false).await;
    for suffix in ["0", "a", "b", "c", "d"] {
        let entity_id = format!("entity:community-page-{suffix}");
        common::seed_entity(
            &memory.db_client,
            "org",
            &entity_id,
            "person",
            &entity_id,
            &[],
        )
        .await;
    }
    let source = "entity:community-page-a";
    for suffix in ["b", "c", "d"] {
        seed_active_edge(
            &memory.db_client,
            &format!("edge:community-page-{suffix}"),
            source,
            &format!("entity:community-page-{suffix}"),
        )
        .await;
    }
    let store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");
    let cutoff = "2027-01-01T00:00:00Z";
    let first_page = store
        .select_edges_filtered_page(cutoff, 0, 2)
        .await
        .expect("first community edge page")
        .iter()
        .map(|edge| logical_edge_id(edge).expect("edge identity"))
        .collect::<HashSet<_>>();
    seed_active_edge(
        &memory.db_client,
        "edge:community-page-0",
        source,
        "entity:community-page-0",
    )
    .await;
    let second_page = store
        .select_edges_filtered_page(cutoff, 2, 2)
        .await
        .expect("second community edge page after insert")
        .iter()
        .map(|edge| logical_edge_id(edge).expect("edge identity"))
        .collect::<HashSet<_>>();

    assert_eq!(first_page.len(), 2);
    assert!(
        first_page.intersection(&second_page).next().is_some(),
        "an earlier inserted edge shifts the offset and repeats a row"
    );
}

#[tokio::test]
async fn community_offset_page_skips_an_edge_invalidated_after_page_one() {
    let memory = common::TestMemory::new(false).await;
    for suffix in ["a", "b", "c", "d"] {
        let entity_id = format!("entity:community-invalidate-{suffix}");
        common::seed_entity(
            &memory.db_client,
            "org",
            &entity_id,
            "person",
            &entity_id,
            &[],
        )
        .await;
    }
    let source = "entity:community-invalidate-a";
    for suffix in ["b", "c", "d"] {
        seed_active_edge(
            &memory.db_client,
            &format!("edge:community-invalidate-{suffix}"),
            source,
            &format!("entity:community-invalidate-{suffix}"),
        )
        .await;
    }
    let store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");
    let cutoff = Utc::now() + chrono::Duration::minutes(1);
    let cutoff = cutoff.to_rfc3339();
    let first_page = store
        .select_edges_filtered_page(&cutoff, 0, 2)
        .await
        .expect("first community edge page")
        .iter()
        .map(|edge| logical_edge_id(edge).expect("edge identity"))
        .collect::<HashSet<_>>();
    memory
        .db_client
        .update(
            "edge:community-invalidate-b",
            json!({
                "t_invalid": Utc::now().to_rfc3339(),
                "t_invalid_ingested": Utc::now().to_rfc3339(),
            }),
            "org",
            &["t_invalid", "t_invalid_ingested"],
        )
        .await
        .expect("invalidate first-page edge");
    let second_page = store
        .select_edges_filtered_page(&cutoff, 2, 2)
        .await
        .expect("second community edge page after invalidation")
        .iter()
        .map(|edge| logical_edge_id(edge).expect("edge identity"))
        .collect::<HashSet<_>>();
    let full_visible = store
        .select_edges_filtered_page(&cutoff, 0, 10)
        .await
        .expect("all currently visible community edges")
        .iter()
        .map(|edge| logical_edge_id(edge).expect("edge identity"))
        .collect::<HashSet<_>>();
    let observed = first_page
        .union(&second_page)
        .cloned()
        .collect::<HashSet<_>>();
    let missing = full_visible
        .difference(&observed)
        .cloned()
        .collect::<Vec<_>>();

    assert_eq!(first_page.len(), 2);
    assert!(
        missing
            .iter()
            .any(|edge| edge.ends_with("entity:community-invalidate-d")),
        "deleting a prior row shifts START and skips a later edge: missing={missing:?}"
    );
}
