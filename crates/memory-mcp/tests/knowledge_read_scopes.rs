//! Knowledge read policy: application-facing reads are
//! owner-named operations, never a caller-supplied table.

use std::sync::Mutex;

use memory_mcp::MemoryError;
use memory_mcp::knowledge::api::{
    KnowledgeReadPort, KnowledgeReadScope, owned_episode_scan, owned_fact_scan,
    read_through_knowledge_port,
};

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
