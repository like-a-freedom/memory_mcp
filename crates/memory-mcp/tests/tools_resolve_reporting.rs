use std::sync::Arc;

use memory_mcp::MemoryService;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use memory_mcp::tools::params::ResolveParams;
use tempfile::TempDir;

#[tokio::test]
async fn resolve_persists_aliases_and_logs_correlated_public_events() {
    let temp_dir = TempDir::new().expect("owned log directory");
    let log_path = temp_dir.path().join("resolve.log");
    memory_mcp::logging::install_log_file(log_path.to_str().expect("UTF-8 log path"))
        .expect("install isolated log sink");

    let namespaces = vec!["org".to_string()];
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(
            "tools_resolve_reporting",
            &namespaces,
            "warn",
        )
        .await
        .expect("in-memory database"),
    );
    db.apply_migrations("org")
        .await
        .expect("apply memory schema");
    let service = MemoryService::new(db, "org".to_string(), "info".to_string(), 50, 100)
        .expect("memory service");

    let canonical = memory_mcp::tools::resolve::resolve(
        &service,
        ResolveParams {
            entity_type: "person".to_string(),
            canonical_name: "Ada Lovelace".to_string(),
            aliases: vec!["Ada".to_string()],
        },
    )
    .await
    .expect("canonical entity resolves");
    assert_eq!(canonical.status, "success");

    let alias = memory_mcp::tools::resolve::resolve(
        &service,
        ResolveParams {
            entity_type: "person".to_string(),
            canonical_name: "Ada".to_string(),
            aliases: Vec::new(),
        },
    )
    .await
    .expect("persisted alias resolves");
    assert_eq!(alias.result, canonical.result);

    let log = std::fs::read_to_string(log_path).expect("read captured public log output");
    let start = log
        .lines()
        .find(|line| line.contains("op=resolve.start") && line.contains("Ada Lovelace"))
        .expect("canonical resolution start is logged");
    let done = log
        .lines()
        .find(|line| line.contains("op=resolve.done") && line.contains(&canonical.result))
        .expect("canonical resolution completion is logged");
    assert!(
        log.find(start).expect("start position") < log.find(done).expect("done position"),
        "resolution must start before completion"
    );
    let alias_start = log
        .lines()
        .filter(|line| line.contains("op=resolve.start"))
        .nth(1)
        .expect("alias resolution start is logged");
    let alias_done = log
        .lines()
        .find(|line| {
            line.contains("op=resolve.done") && request_id(line) == request_id(alias_start)
        })
        .expect("alias resolution completion is logged");
    assert!(
        log.find(done).expect("canonical completion position")
            < log.find(alias_start).expect("alias start position"),
        "the second operation must start after the first completed"
    );
    assert!(
        log.find(alias_start).expect("alias start position")
            < log.find(alias_done).expect("alias completion position")
    );
    let start_id = request_id(start);
    let done_id = request_id(done);
    assert_eq!(start_id, done_id, "one resolve call keeps a correlated ID");
    let numeric_id = |id: &str| {
        id.strip_prefix("req_")
            .expect("request prefix")
            .parse::<u64>()
            .expect("numeric request ID")
    };
    assert_eq!(
        numeric_id(request_id(alias_start)),
        numeric_id(start_id) + 1
    );
    assert!(
        start_id
            .strip_prefix("req_")
            .and_then(|digits| digits.parse::<u64>().ok())
            .is_some_and(|number| number > 0),
        "request IDs carry a positive numeric value: {start_id}"
    );
}

fn request_id(line: &str) -> &str {
    line.split_whitespace()
        .find_map(|field| field.strip_prefix("req="))
        .filter(|request_id| request_id.starts_with("req_"))
        .expect("log line carries a request ID")
}
