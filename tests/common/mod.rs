use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use memory_mcp::service::{MemoryError, MemoryService};
use memory_mcp::storage::DbClient;

#[derive(Default, Clone)]
struct FakeNamespaceStore {
    tables: HashMap<String, HashMap<String, Value>>,
    counters: HashMap<String, i32>,
}

#[derive(Default)]
pub struct FakeDbClient {
    namespaces: Mutex<HashMap<String, FakeNamespaceStore>>,
}

impl FakeDbClient {
    fn store(&self, namespace: &str) -> FakeNamespaceStore {
        self.namespaces
            .lock()
            .unwrap()
            .get(namespace)
            .cloned()
            .unwrap_or_default()
    }

    fn set_store(&self, namespace: &str, store: FakeNamespaceStore) {
        self.namespaces
            .lock()
            .unwrap()
            .insert(namespace.to_string(), store);
    }

    fn table_from_id(record_id: &str) -> String {
        record_id.split(':').next().unwrap_or(record_id).to_string()
    }
}

#[async_trait]
impl DbClient for FakeDbClient {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        let store = self.store(namespace);
        let table = Self::table_from_id(record_id);
        Ok(store
            .tables
            .get(&table)
            .and_then(|table| table.get(record_id).cloned()))
    }

    async fn select_table(&self, table: &str, namespace: &str) -> Result<Vec<Value>, MemoryError> {
        let store = self.store(namespace);
        Ok(store
            .tables
            .get(table)
            .map(|table| table.values().cloned().collect())
            .unwrap_or_default())
    }

    async fn create(
        &self,
        record_id: &str,
        mut content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        let mut store = self.store(namespace);
        let mut record_id = record_id.to_string();
        let table = if record_id.contains(':') {
            Self::table_from_id(&record_id)
        } else {
            let counter = store.counters.entry(record_id.clone()).or_insert(0);
            *counter += 1;
            let table = record_id.clone();
            record_id = format!("{table}:{}", counter);
            table
        };
        if let Value::Object(ref mut map) = content {
            map.insert("id".to_string(), Value::String(record_id.clone()));
        }
        store
            .tables
            .entry(table)
            .or_default()
            .insert(record_id.clone(), content.clone());
        self.set_store(namespace, store);
        Ok(content)
    }

    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        let mut store = self.store(namespace);
        let table = Self::table_from_id(record_id);
        let entry = store
            .tables
            .entry(table)
            .or_default()
            .entry(record_id.to_string())
            .or_insert(Value::Object(Default::default()));
        if let (Value::Object(map), Value::Object(update)) = (entry, content.clone()) {
            for (key, value) in update {
                map.insert(key, value);
            }
            map.insert("id".to_string(), Value::String(record_id.to_string()));
        }
        self.set_store(namespace, store);
        Ok(content)
    }

    async fn query(
        &self,
        _sql: &str,
        _vars: Option<Value>,
        _namespace: &str,
    ) -> Result<Value, MemoryError> {
        Ok(Value::Null)
    }

    async fn select_facts_filtered(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        // Simple in-memory filtering for tests
        let store = self.store(namespace);
        let facts = store
            .tables
            .get("fact")
            .map(|t| t.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        let query_lower = query_contains.map(|q| q.to_lowercase());

        let mut filtered: Vec<Value> = facts
            .into_iter()
            .filter(|f| {
                let f_scope = f.get("scope").and_then(Value::as_str).unwrap_or_default();
                let t_valid = f.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                let t_ingested = f
                    .get("t_ingested")
                    .and_then(Value::as_str)
                    .unwrap_or(t_valid);
                let t_invalid = f.get("t_invalid").and_then(Value::as_str);
                let t_invalid_ingested = f.get("t_invalid_ingested").and_then(Value::as_str);

                // Scope match
                if f_scope != scope {
                    return false;
                }
                // t_valid <= cutoff
                if t_valid > cutoff {
                    return false;
                }
                if t_ingested > cutoff {
                    return false;
                }
                // t_invalid IS NULL OR t_invalid > cutoff OR t_invalid_ingested > cutoff
                if let Some(inv) = t_invalid
                    && !inv.is_empty()
                    && inv <= cutoff
                {
                    let invalid_known = t_invalid_ingested
                        .map(|ingested| !ingested.is_empty() && ingested <= cutoff)
                        .unwrap_or(true);
                    if invalid_known {
                        return false;
                    }
                }
                // Query matching: per-word OR (mirrors SurrealDB full-text search semantics)
                if let Some(ref q) = query_lower {
                    let content = f
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase();
                    let words: Vec<&str> = q.split_whitespace().filter(|w| w.len() >= 2).collect();
                    if words.is_empty() {
                        if !content.contains(q) {
                            return false;
                        }
                    } else {
                        let any_match = words.iter().any(|w| content.contains(w));
                        if !any_match {
                            return false;
                        }
                    }
                }
                true
            })
            .collect();

        // Sort by t_valid DESC
        filtered.sort_by(|a, b| {
            let ta = a.get("t_valid").and_then(Value::as_str).unwrap_or_default();
            let tb = b.get("t_valid").and_then(Value::as_str).unwrap_or_default();
            tb.cmp(ta)
        });

        // Limit
        filtered.truncate(limit.max(1) as usize);

        Ok(filtered)
    }

    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let store = self.store(namespace);
        let edges = store
            .tables
            .get("edge")
            .map(|t| t.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        let mut filtered: Vec<Value> = edges
            .into_iter()
            .filter(|e| {
                let t_valid = e.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                let t_ingested = e
                    .get("t_ingested")
                    .and_then(Value::as_str)
                    .unwrap_or(t_valid);
                let t_invalid = e.get("t_invalid").and_then(Value::as_str);
                let t_invalid_ingested = e.get("t_invalid_ingested").and_then(Value::as_str);

                if t_valid > cutoff {
                    return false;
                }
                if t_ingested > cutoff {
                    return false;
                }
                if let Some(inv) = t_invalid
                    && !inv.is_empty()
                    && inv <= cutoff
                {
                    let invalid_known = t_invalid_ingested
                        .map(|ingested| !ingested.is_empty() && ingested <= cutoff)
                        .unwrap_or(true);
                    if invalid_known {
                        return false;
                    }
                }
                true
            })
            .collect();

        filtered.sort_by(|a, b| {
            let from_a = a.get("from_id").and_then(Value::as_str).unwrap_or_default();
            let from_b = b.get("from_id").and_then(Value::as_str).unwrap_or_default();
            let to_a = a.get("to_id").and_then(Value::as_str).unwrap_or_default();
            let to_b = b.get("to_id").and_then(Value::as_str).unwrap_or_default();
            from_a.cmp(from_b).then_with(|| to_a.cmp(to_b))
        });

        Ok(filtered)
    }

    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        // Fake client: no-op for migrations in tests
        Ok(())
    }
}

pub fn make_service() -> MemoryService {
    let client = FakeDbClient::default();
    MemoryService::new(
        Arc::new(client),
        vec!["test".to_string()],
        "info".to_string(),
        50,
        100,
    )
    .expect("service init")
}

pub async fn setup_embedded_service() -> Result<(tempfile::TempDir, MemoryService), MemoryError> {
    use memory_mcp::config::SurrealConfig;
    use memory_mcp::storage::SurrealDbClient;

    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let data_dir = tmp.path().to_str().unwrap().to_string();
    let config = SurrealConfig {
        db_name: "example".to_string(),
        url: None,
        namespaces: vec!["example".to_string()],
        username: "root".to_string(),
        password: "root".to_string(),
        log_level: "warn".to_string(),
        embedded: true,
        data_dir: Some(data_dir),
    };

    let default = config.namespaces[0].clone();
    let db_client = SurrealDbClient::connect(&config, &default).await?;
    db_client.apply_migrations(&default).await?;

    let service = MemoryService::new(
        Arc::new(db_client),
        config.namespaces.clone(),
        config.log_level.clone(),
        50,
        100,
    )?;
    Ok((tmp, service))
}
