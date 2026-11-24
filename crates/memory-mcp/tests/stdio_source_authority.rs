//! Process-level source-authority contract for the real stdio MCP binary.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

struct StdioServer {
    child: Child,
    stdin: ChildStdin,
    responses: mpsc::Receiver<String>,
    temp: TempDir,
    log_path: std::path::PathBuf,
}

impl StdioServer {
    fn start() -> Self {
        let temp = tempfile::tempdir().expect("temporary server directory");
        let log_path = temp.path().join("server.log");
        let binary = env!("CARGO_BIN_EXE_memory_mcp");
        let mut child = Command::new(binary)
            .env_clear()
            .arg("serve")
            .env("SURREALDB_EMBEDDED", "true")
            .env("SURREALDB_DATA_DIR", temp.path().join("db"))
            .env("SURREALDB_DB_NAME", "stdio_source_authority")
            .env("SURREALDB_NAMESPACE", "org")
            .env("SURREALDB_USERNAME", "root")
            .env("SURREALDB_PASSWORD", "root")
            .env("EMBEDDINGS_ENABLED", "false")
            .env("NER_EXTRACTOR", "regex")
            .env("MEMORY_LOG_FILE", &log_path)
            .env("RUST_LOG", "trace")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start stdio MCP server");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (sender, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = sender.send(line);
            }
        });

        Self {
            child,
            stdin,
            responses,
            temp,
            log_path,
        }
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{request}").expect("write JSON-RPC request");
        self.stdin.flush().expect("flush JSON-RPC request");

        loop {
            let line = self
                .responses
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|error| {
                    let status = self.child.try_wait().ok().flatten();
                    panic!("waiting for MCP response {id}: {error}; child={status:?}");
                });
            let response: Value = serde_json::from_str(&line).expect("valid JSON-RPC response");
            if response.get("id").and_then(Value::as_i64) == Some(id) {
                return response;
            }
        }
    }

    fn call_tool(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    fn initialize(&mut self) {
        let response = self.request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "source-authority-test", "version": "1"}
            }),
        );
        assert!(response.get("error").is_none(), "initialize: {response}");
    }

    fn ingest_and_extract(
        &mut self,
        id: i64,
        reference: &str,
        source_id: &str,
        source_group: &str,
        policy_tag: &str,
    ) -> Value {
        let ingested = self.call_tool(
            id,
            "ingest",
            json!({
                "source_type": "agent_note",
                "source_id": source_id,
                "content": reference,
                "t_ref": format!("2020-01-01T12:{:02}:00Z", id),
                "policy_tags": [policy_tag],
                "source_group": source_group,
                "source_position": id
            }),
        );
        assert!(
            ingested.get("error").is_none(),
            "ingest {reference}: {ingested}"
        );
        let episode_id = ingested
            .pointer("/result/structuredContent/result")
            .and_then(Value::as_str)
            .expect("ingest returns stable episode id")
            .to_owned();

        let extracted = self.call_tool(id + 1, "extract", json!({"episode_id": episode_id}));
        assert!(
            extracted.get("error").is_none(),
            "extract {reference}: {extracted}"
        );
        assert_eq!(
            extracted.pointer("/result/structuredContent/status"),
            Some(&json!("success")),
            "extract must derive the literal note"
        );
        extracted
    }

    fn close(mut self) -> String {
        drop(self.stdin);
        let deadline = std::time::Instant::now() + Duration::from_secs(35);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(None) => panic!("stdio server did not exit in the bounded shutdown window"),
                Err(error) => panic!("poll stdio server exit: {error}"),
            }
        }
        std::fs::read_to_string(&self.log_path).expect("read captured server log")
    }
}

#[test]
fn stdio_ingest_preserves_path_and_url_without_reading_or_fetching_them() {
    let mut server = StdioServer::start();
    server.initialize();

    let private_file = server.temp.path().join("private-source-canary.txt");
    let private_contents = "local file data must not replace the caller's reference";
    std::fs::write(&private_file, private_contents).expect("write controlled file");
    let literal_path = private_file.to_string_lossy().into_owned();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind URL canary");
    listener
        .set_nonblocking(true)
        .expect("set URL canary nonblocking");
    let literal_url = format!(
        "http://{}/stdio-reference",
        listener.local_addr().expect("URL canary address")
    );

    server.ingest_and_extract(
        2,
        &literal_path,
        "stdio-source-id-path-canary",
        "stdio-source-group-path-canary",
        "stdio-policy-tag-path-canary",
    );
    server.ingest_and_extract(
        4,
        &literal_url,
        "stdio-source-id-url-canary",
        "stdio-source-group-url-canary",
        "stdio-policy-tag-url-canary",
    );

    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "literal URL ingestion must not connect to its loopback target"
    );

    let path_recall = server.call_tool(
        6,
        "assemble_context",
        json!({
            "query": format!("Which exact source path should I remember? {literal_path}"),
            "fact_types": [],
            "valid_at": "2026-11-22T12:00:00Z",
            "known_at": "2099-01-01T00:00:00Z",
            "budget": 5,
            "compact": true
        }),
    );
    assert!(
        path_recall
            .pointer("/result/structuredContent/result")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item["content"]
                .as_str()
                .is_some_and(|content| content.contains(&literal_path)))),
        "stdio recall should retain the exact path: {path_recall}"
    );
    assert!(!path_recall.to_string().contains(private_contents));

    let url_recall = server.call_tool(
        7,
        "assemble_context",
        json!({
            "query": format!("Which exact URL should I remember as a reference? {literal_url}"),
            "fact_types": [],
            "valid_at": "2026-11-22T12:00:00Z",
            "known_at": "2099-01-01T00:00:00Z",
            "budget": 5,
            "compact": true
        }),
    );
    assert!(
        url_recall
            .pointer("/result/structuredContent/result")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item["content"]
                .as_str()
                .is_some_and(|content| content.contains(&literal_url)))),
        "stdio recall should retain the exact URL: {url_recall}"
    );
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "recall must not fetch the literal URL"
    );

    let secret = "stdio-source-authority-credential-canary";
    let refused = server.call_tool(
        8,
        "ingest",
        json!({
            "source_type": "agent_note",
            "source_id": "stdio-credential-refusal",
            "content": format!("API_KEY={secret}"),
            "t_ref": "2020-01-01T12:03:00Z",
            "policy_tags": []
        }),
    );
    assert!(
        refused.get("error").is_some() || refused["result"]["isError"] == true,
        "stdio should refuse high-confidence credential material: {refused}"
    );
    assert!(!refused.to_string().contains(secret));

    let inline_secret = "stdio-inline-extract-credential-canary";
    let inline_refused = server.call_tool(
        9,
        "extract",
        json!({
            "content": format!("API_KEY={inline_secret}"),
            "source_type": "agent_note",
            "source_id": "stdio-inline-extract-source-id-canary",
            "t_ref": "2020-01-01T12:04:00Z"
        }),
    );
    assert!(
        inline_refused.get("error").is_some() || inline_refused["result"]["isError"] == true,
        "inline extract must refuse credential-bearing content: {inline_refused}"
    );
    assert!(!inline_refused.to_string().contains(inline_secret));

    let episode_id_canary = "episode:stdio-extract-episode-id-canary";
    let missing_episode = server.call_tool(10, "extract", json!({"episode_id": episode_id_canary}));
    assert!(
        missing_episode.get("error").is_some() || missing_episode["result"]["isError"] == true,
        "unknown episode should be reported as an error: {missing_episode}"
    );

    let entity_name_canary = "stdio-resolve-name-canary";
    let _resolved = server.call_tool(
        11,
        "resolve",
        json!({
            "entity_type": "person",
            "canonical_name": entity_name_canary,
            "aliases": ["stdio-resolve-alias-canary"]
        }),
    );

    let fact_id_canary = "fact:stdio-invalidate-fact-id-canary";
    let missing_fact = server.call_tool(
        12,
        "invalidate",
        json!({
            "fact_id": fact_id_canary,
            "reason": "stdio-invalidate-reason-canary",
            "t_invalid": "2020-01-01T12:05:00Z"
        }),
    );
    assert!(
        missing_fact.get("error").is_some() || missing_fact["result"]["isError"] == true,
        "unknown fact should be reported as an error: {missing_fact}"
    );

    let malformed_episode_id = "stdio-malformed-episode-id-canary";
    let malformed_episode =
        server.call_tool(13, "extract", json!({"episode_id": malformed_episode_id}));
    assert!(
        malformed_episode.get("error").is_some() || malformed_episode["result"]["isError"] == true,
        "malformed episode ID should be reported as an error: {malformed_episode}"
    );
    assert!(
        malformed_episode
            .to_string()
            .contains("missing its table prefix"),
        "the caller should retain the actionable validation detail: {malformed_episode}"
    );
    assert!(
        !malformed_episode.to_string().contains(malformed_episode_id),
        "the validation response must not echo the malformed caller ID: {malformed_episode}"
    );

    let ordinary_query_canary = "stdio-ordinary-query-content-canary";
    let ordinary_context = server.call_tool(
        14,
        "assemble_context",
        json!({
            "query": ordinary_query_canary,
            "fact_types": [],
            "budget": 5,
            "compact": true
        }),
    );
    assert!(
        ordinary_context.get("error").is_none(),
        "ordinary current recall should succeed: {ordinary_context}"
    );

    let logs = server.close();
    assert!(!logs.contains(&literal_path));
    assert!(!logs.contains(private_contents));
    assert!(!logs.contains(&literal_url));
    assert!(!logs.contains(secret));
    assert!(!logs.contains(inline_secret));
    assert!(!logs.contains("stdio-inline-extract-source-id-canary"));
    assert!(
        !logs.contains(episode_id_canary),
        "caller episode ID appeared in logs: {}",
        logs.lines()
            .filter(|line| line.contains(episode_id_canary))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(!logs.contains(entity_name_canary));
    assert!(!logs.contains("stdio-resolve-alias-canary"));
    assert!(!logs.contains(fact_id_canary));
    assert!(
        !logs.contains(malformed_episode_id),
        "malformed caller ID appeared in transport logs: {}",
        logs.lines()
            .filter(|line| line.contains(malformed_episode_id))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(!logs.contains("stdio-invalidate-reason-canary"));
    assert!(!logs.contains("stdio-source-id-path-canary"));
    assert!(!logs.contains("stdio-source-id-url-canary"));
    assert!(!logs.contains("stdio-source-group-path-canary"));
    assert!(!logs.contains("stdio-source-group-url-canary"));
    assert!(!logs.contains("stdio-policy-tag-path-canary"));
    assert!(!logs.contains("stdio-policy-tag-url-canary"));
    assert!(
        !logs.contains(ordinary_query_canary),
        "ordinary query appeared in retrieval logs: {}",
        logs.lines()
            .filter(|line| line.contains(ordinary_query_canary))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
