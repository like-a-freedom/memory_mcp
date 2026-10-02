//! Knowledge read policy: application-facing reads are
//! owner-named operations, never a caller-supplied table.

use std::sync::Mutex;

use chrono::Utc;

use memory_mcp::MemoryError;
use memory_mcp::knowledge::api::{
    KnowledgeReadPort, KnowledgeReadScope, owned_episode_scan, owned_fact_scan,
    read_through_knowledge_port,
};

mod common;

/// Records what was asked for so the test can prove the
/// application layer cannot express an arbitrary table.
struct RecordingPort {
    requested: Mutex<Vec<KnowledgeReadScope>>,
    refuse: Option<KnowledgeReadScope>,
}

impl RecordingPort {
    fn serving() -> Self {
        Self {
            requested: Mutex::new(Vec::new()),
            refuse: None,
        }
    }

    fn refusing(scope: KnowledgeReadScope) -> Self {
        Self {
            requested: Mutex::new(Vec::new()),
            refuse: Some(scope),
        }
    }

    fn requested(&self) -> Vec<KnowledgeReadScope> {
        self.requested.lock().expect("requested lock").clone()
    }
}

#[async_trait::async_trait]
impl KnowledgeReadPort for RecordingPort {
    async fn read_scope(
        &self,
        scope: KnowledgeReadScope,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        self.requested.lock().expect("requested lock").push(scope);
        if self.refuse == Some(scope) {
            return Err(MemoryError::Storage("scope unavailable".into()));
        }
        Ok(Vec::new())
    }
}

#[test]
fn a_read_scope_is_named_by_owner_not_by_a_table_string() {
    let facts = KnowledgeReadScope::Facts;
    let episodes = KnowledgeReadScope::Episodes;

    assert_ne!(facts, episodes);
    assert_eq!(facts.owner(), "knowledge");
    assert_eq!(episodes.owner(), "memory");
}

#[tokio::test]
async fn an_owned_fact_scan_reaches_the_port_as_a_fact_scope() {
    let port = RecordingPort::serving();

    owned_fact_scan(&port)
        .await
        .expect("the fact scope is served");

    assert_eq!(port.requested(), vec![KnowledgeReadScope::Facts]);
}

#[tokio::test]
async fn an_owned_episode_scan_reaches_the_port_as_an_episode_scope() {
    let port = RecordingPort::serving();

    owned_episode_scan(&port)
        .await
        .expect("the episode scope is served");

    assert_eq!(port.requested(), vec![KnowledgeReadScope::Episodes]);
}

#[tokio::test]
async fn a_port_error_propagates_to_the_caller() {
    let port = RecordingPort::refusing(KnowledgeReadScope::Facts);

    assert!(owned_fact_scan(&port).await.is_err());
}

#[tokio::test]
async fn a_typed_read_reports_the_owner_that_answered_it() {
    let port = RecordingPort::serving();

    let read = read_through_knowledge_port(&port, KnowledgeReadScope::Facts)
        .await
        .expect("read succeeds");

    assert_eq!(read.owner, "knowledge");
    assert_eq!(read.scope, KnowledgeReadScope::Facts);
    assert_eq!(read.rows, 0);
}

/// The storage platform names no domain table.
///
/// `storage` connects to a database. It does not know what a `claim` is, and
/// the module doc said so while `queries.rs` built SQL for `fact`, `edge`,
/// `community` and `episode` and `client.rs` kept the allowlist. The claim and
/// the code were both true and they cancelled out.
///
/// The tables checked here are the nineteen the migrations create that are not
/// the platform's own four. A string literal naming one inside `storage/` is
/// SQL for a domain the platform has no business knowing, and the fix is to
/// move the builder to the context that owns the concept — not to allow the
/// platform to keep a copy.
#[test]
fn the_storage_platform_names_no_domain_table() {
    use std::collections::BTreeSet;

    let platform_tables: BTreeSet<&str> = ["event_log", "query_log", "script_migration", "task"]
        .into_iter()
        .collect();

    let domain_tables: Vec<&str> = memory_mcp::storage::expected_schema_tables()
        .iter()
        .copied()
        .filter(|table| !platform_tables.contains(table))
        .collect();
    assert!(
        domain_tables.len() >= 19,
        "expected at least nineteen domain tables, found {}",
        domain_tables.len()
    );

    let storage_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/storage");
    let mut offenders: Vec<String> = Vec::new();
    let mut stack = vec![storage_dir];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
                continue;
            }
            if !path.extension().is_some_and(|ext| ext == "rs") {
                continue;
            }
            // Two places in `storage/` legitimately name a domain table and
            // must keep doing so: `table_scope.rs`, whose whole job is the
            // partition between contexts, and the `#[cfg(test)]` modules. The
            // claim is about production code that builds SQL for a domain.
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name == "table_scope.rs" {
                continue;
            }
            // `migrations.rs` is the one production file in `storage/` that
            // must name every table: it is what creates them. That is the
            // difference between it and the rest of the module — it owns the
            // schema, not a domain.
            if name == "migrations.rs" {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // The `#[cfg(test)]` module at the end of the file is not
            // production code, and a test may name any table it likes.
            let text = match text.find("#[cfg(test)]") {
                Some(at) => &text[..at],
                None => &text[..],
            };
            let relative = path
                .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap_or(&path)
                .display()
                .to_string();
            for (index, line) in text.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.is_empty() {
                    continue;
                }
                for table in &domain_tables {
                    if trimmed.contains(&format!("\"{table}\""))
                        || trimmed.contains(&format!("'{table}'"))
                        || trimmed.contains(&format!("FROM {table}"))
                        || trimmed.contains(&format!(" type::record('{table}'"))
                    {
                        offenders.push(format!("{relative}:{}: {}", index + 1, trimmed));
                    }
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "{} line(s) in src/storage/ name a domain table. The platform connects \
         and migrates; the SQL for a domain belongs to the context that owns the \
         domain:\n\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// `relate` used to hardcode `EdgeOrigin::Inferred`, strength `1.0`,
/// confidence `0.8` and `Provenance::manual()` — a business decision hidden
/// in a fixture helper, with no way for a caller to state anything else.
///
/// This asserts an explicitly-originated edge round-trips with the values it
/// was given. It cannot be written before the signature changed, which is
/// the proof the hardcoding was a real constraint rather than a default.
#[tokio::test]
async fn relate_records_an_operator_originated_edge() {
    use memory_mcp::models::{EdgeOrigin, Provenance};
    use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;

    let (service, db_client) = common::make_service_with_client().await;

    let episode_id = IngestCapability::ingest_from_service(
        &service,
        memory_mcp::models::IngestRequest {
            source_type: "test".to_string(),
            source_id: "relate-origin".to_string(),
            content: "Alice Smith and Bob Jones presented at a conference".to_string(),
            t_ref: Utc::now(),
            t_ingested: None,
            policy_tags: Vec::new(),
        },
        None,
    )
    .await
    .expect("ingest");

    let extracted = memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability::extract_from_service(
        &service, &episode_id, None, None,
    )
    .await
    .expect("extract");

    let alice = extracted
        .entities
        .iter()
        .find(|e| e.canonical_name.to_lowercase().contains("alice"))
        .map(|e| e.entity_id.clone())
        .expect("alice entity");
    let bob = extracted
        .entities
        .iter()
        .find(|e| e.canonical_name.to_lowercase().contains("bob"))
        .map(|e| e.entity_id.clone())
        .expect("bob entity");

    let attributes = memory_mcp::models::EdgeAttributes {
        origin: EdgeOrigin::Ambiguous,
        strength: 0.42,
        confidence: 0.13,
        provenance: Provenance::manual(),
    };
    service
        .relate(&alice, "attended_with", &bob, attributes)
        .await
        .expect("relate");

    let store =
        memory_mcp::knowledge::KnowledgeGraphStore::new(db_client.clone(), "org".to_string());
    let edges = store
        .select_edges_filtered_page(&Utc::now().to_rfc3339(), 0, 50)
        .await
        .expect("read edges");

    let edge = edges
        .iter()
        .find(|row| {
            row.get("relation")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|r| r == "attended_with")
        })
        .expect("the edge just written");

    assert_eq!(
        edge.get("origin").and_then(serde_json::Value::as_str),
        Some("ambiguous")
    );
    assert_eq!(
        edge.get("confidence").and_then(serde_json::Value::as_f64),
        Some(0.13),
        "the caller stated the confidence; the helper must not overwrite it"
    );
    assert_eq!(
        edge.get("strength").and_then(serde_json::Value::as_f64),
        Some(0.42)
    );
}

/// Task 4.2 deleted `temporal_field_names_for_table` and gave each context a
/// `*_TEMPORAL_FIELDS` constant. That moves the answer, it does not answer the
/// question: a constant can name a column the table does not have, and a
/// missing entry silently stops coercing a datetime to a `datetime`.
///
/// `started_at` was in `INBOX_REVISION_TEMPORAL_FIELDS` and `inbox_revision`
/// has no such column; `processed_at`, which the table does have, was
/// missing. Nothing caught it — the wrong name is inert, so the write only
/// failed where the column is actually written.
///
/// This test parses the migrations — the only place the schema is declared —
/// and asserts every named column exists on the table the constant belongs
/// to.
#[test]
fn every_temporal_field_list_names_only_real_columns() {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::Path;

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Schema: `DEFINE FIELD <col> ON <table>` across every migration, plus the
    // Rust-side column lists the postconditions check.
    let mut schema: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut migrations: Vec<_> = fs::read_dir(manifest.join("migrations"))
        .expect("migrations directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("surql"))
        .collect();
    migrations.sort();
    for path in migrations {
        let text = fs::read_to_string(&path).expect("readable migration");
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("DEFINE FIELD ") else {
                continue;
            };
            let rest = rest.strip_prefix("OVERWRITE ").unwrap_or(rest);
            // `DEFINE FIELD <col> ON [TABLE] <table> TYPE …`
            let mut parts = rest.split_whitespace();
            let Some(column) = parts.next() else {
                continue;
            };
            let Some(on) = parts.next() else {
                continue;
            };
            if on != "ON" {
                continue;
            }
            let mut next = parts.next();
            if next == Some("TABLE") {
                next = parts.next();
            }
            let Some(table) = next else {
                continue;
            };
            schema
                .entry(table.to_ascii_lowercase())
                .or_default()
                .insert(column.to_string());
        }
    }

    assert!(
        schema.len() >= 20,
        "expected the migrations to declare at least twenty tables, parsed {}",
        schema.len()
    );

    let query_files = [
        "src/knowledge/queries.rs",
        "src/memory/queries.rs",
        "src/embedding/queries.rs",
    ];
    let mut checked = 0usize;
    let mut problems: Vec<String> = Vec::new();
    for file in query_files {
        let text = fs::read_to_string(manifest.join(file)).expect("readable queries module");
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if !trimmed.starts_with("pub const ") || !trimmed.contains("TEMPORAL_FIELDS") {
                continue;
            }
            let name = trimmed
                .trim_start_matches("pub const ")
                .split(':')
                .next()
                .expect("constant name")
                .to_string();
            // The table is the constant name minus its
            // `*_TEMPORAL_FIELDS` suffix: `FACT_…` → `fact`,
            // `INBOX_REVISION_…` → `inbox_revision`.
            let table = name
                .strip_suffix("_TEMPORAL_FIELDS")
                .expect("suffix")
                .to_ascii_lowercase();
            // Read the list body, which may span lines.
            let mut body = String::new();
            let mut cursor = index;
            while cursor < text.lines().count() {
                let l = text.lines().nth(cursor).unwrap_or_default();
                body.push_str(l);
                if l.trim_end().ends_with(';') {
                    break;
                }
                cursor += 1;
            }
            let columns = schema.get(&table).unwrap_or_else(|| {
                panic!("{name} names a table the schema does not define: {table}")
            });
            for column in body
                .split('"')
                .skip(1)
                .step_by(2)
                .filter(|s| s.contains('_') || s.starts_with('t'))
            {
                checked += 1;
                if !columns.contains(column) {
                    problems.push(format!(
                        "  {file}:{} {name} names `{column}`, which table `{table}` does not have",
                        index + 1
                    ));
                }
            }
        }
    }

    // The platform's own constant, against the table it names. It is the same
    // kind of list and it went wrong the same way: `lease_expires_at`,
    // `started_at` and `completed_at` are not columns of `script_migration`,
    // which is SCHEMAFULL over three fields.
    {
        let file = "src/storage/migrations.rs";
        let table = "script_migration";
        let text = fs::read_to_string(manifest.join(file)).expect("readable migrations module");
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains("TEMPORAL_FIELDS: &[&str]") {
                continue;
            }
            let columns = schema.get(table).unwrap_or_else(|| {
                panic!("{file} names a table the schema does not define: {table}")
            });
            let mut body = String::new();
            let mut cursor = index;
            while cursor < lines.len() {
                body.push_str(lines[cursor]);
                if lines[cursor].trim_end().ends_with(';') {
                    break;
                }
                cursor += 1;
            }
            for column in body.split('"').skip(1).step_by(2) {
                checked += 1;
                if !columns.contains(column) {
                    problems.push(format!(
                        "  {file}:{} names `{column}`, which table `{table}` does not have",
                        index + 1
                    ));
                }
            }
        }
    }

    assert!(
        checked > 20,
        "expected to check more columns, checked {checked}"
    );
    assert!(
        problems.is_empty(),
        "{} temporal-field name(s) do not exist in the schema:\n\n{}\n\n\
         These lists are copied from each other when a table is added, and a \
         name carried over from a neighbour is how `started_at` ended up in \
         `INBOX_REVISION_TEMPORAL_FIELDS` for a table with no such column. \
         Check the migration for the table before adding a column here.",
        problems.len(),
        problems.join("\n")
    );
}
