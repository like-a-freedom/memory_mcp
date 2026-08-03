use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use rmcp::handler::server::wrapper::Parameters;
use tempfile::TempDir;

use memory_mcp::mcp::MemoryMcp;
use memory_mcp::models::{AssembleContextRequest, ExplainRequest};
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use memory_mcp::service::capabilities::explain::ExplainCapability;

mod common;

struct StdioMcpProcess {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<serde_json::Value>,
    _temp_dir: TempDir,
}

impl StdioMcpProcess {
    fn start() -> Self {
        let temp_dir = tempfile::tempdir().expect("create stdio MCP data directory");
        let mut child = Command::new(env!("CARGO_BIN_EXE_memory_mcp"))
            .arg("serve")
            .env("SURREALDB_EMBEDDED", "true")
            .env("SURREALDB_DATA_DIR", temp_dir.path())
            .env("SURREALDB_DB_NAME", "memory_tasks_e2e")
            .env("SURREALDB_NAMESPACES", "org")
            .env("SURREALDB_USERNAME", "root")
            .env("SURREALDB_PASSWORD", "root")
            .env("EMBEDDINGS_ENABLED", "false")
            .env("NER_PROVIDER", "anno")
            .env("RUST_LOG", "warn")
            .env_remove("SURREALDB_URL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start stdio MCP server");
        let stdin = child.stdin.take().expect("server stdin");
        let stdout = child.stdout.take().expect("server stdout");
        let (sender, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(response) = serde_json::from_str(&line) {
                    let _ = sender.send(response);
                }
            }
        });
        Self {
            child,
            stdin,
            responses,
            _temp_dir: temp_dir,
        }
    }

    fn request(&mut self, id: i64, method: &str, params: serde_json::Value) -> serde_json::Value {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{request}").expect("write MCP request");
        self.stdin.flush().expect("flush MCP request");
        loop {
            let response = self
                .responses
                .recv_timeout(Duration::from_secs(20))
                .unwrap_or_else(|error| panic!("timed out waiting for MCP response {id}: {error}"));
            if response.get("id").and_then(serde_json::Value::as_i64) == Some(id) {
                return response;
            }
        }
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{notification}").expect("write MCP notification");
        self.stdin.flush().expect("flush MCP notification");
    }

    fn initialize(&mut self) {
        let response = self.request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "memory-mcp-e2e", "version": "1.0.0"}
            }),
        );
        let tasks = &response["result"]["capabilities"]["tasks"];
        assert!(tasks["list"].is_object(), "missing tasks/list: {response}");
        assert!(
            tasks["cancel"].is_object(),
            "missing tasks/cancel: {response}"
        );
        assert!(
            tasks["requests"]["tools"]["call"].is_object(),
            "missing task-augmented tools/call: {response}"
        );
        self.notify("notifications/initialized", serde_json::json!({}));
    }

    fn wait_for_terminal_task(&mut self, request_id: &mut i64, task_id: &str) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(
                Instant::now() < deadline,
                "task {task_id} did not complete before the E2E deadline"
            );
            let status = self.request(
                *request_id,
                "tasks/get",
                serde_json::json!({"taskId": task_id}),
            );
            *request_id += 1;
            match status["result"]["status"].as_str() {
                Some("completed" | "failed" | "cancelled") => return status,
                Some("working") => std::thread::sleep(Duration::from_millis(10)),
                other => panic!("unexpected task state {other:?}: {status}"),
            }
        }
    }
}

impl Drop for StdioMcpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn task_payload_for_sync_result(
    mut synchronous_result: serde_json::Value,
    task_id: &str,
) -> serde_json::Value {
    let object = synchronous_result
        .as_object_mut()
        .expect("tools/call result object");
    let meta = object
        .entry("_meta")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("tools/call result metadata object");
    meta.insert(
        "io.modelcontextprotocol/related-task".to_string(),
        serde_json::json!({"taskId": task_id}),
    );
    synchronous_result
}

#[tokio::test]
async fn test_mcp_tools_flow() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "MSG-203",
        "content": "I will finish it by Friday. ARR $2M",
        "t_ref": "2026-01-10T00:00:00Z",
        "scope": "org"
    });
    let episode_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest")
        .0;
    assert_eq!(episode_id.status, "success");
    assert_eq!(
        episode_id.guidance.as_deref(),
        Some("Call extract next to derive entities and facts."),
    );
    let episode_id = episode_id.result;

    let extract_params = serde_json::json!({
        "episode_id": episode_id
    });
    let extraction = mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
        .expect("extract")
        .0;
    assert_eq!(extraction.status, "success");
    let extraction = extraction.result;
    assert!(extraction.facts.len() >= 2);

    let assemble_request: AssembleContextRequest = serde_json::from_value(serde_json::json!({
        "query": "ARR",
        "scope": "org",
        "as_of": Utc::now().to_rfc3339(),
        "budget": 5,
        "compact": false,
    }))
    .unwrap();
    let context = AssembleContextCapability::assemble_context(
        &mcp.service().build_context(),
        assemble_request,
    )
    .await
    .expect("assemble");
    assert!(!context.is_empty());

    // Verify rounding of f64 fields in assemble_context response
    let context_json = serde_json::to_value(&context).unwrap();
    if let Some(items) = context_json.as_array() {
        for (i, item) in items.iter().enumerate() {
            let conf = item["confidence"]
                .as_f64()
                .unwrap_or_else(|| panic!("item[{i}] missing 'confidence'"));
            let rounded = (conf * 100.0).round() / 100.0;
            assert!(
                (conf - rounded).abs() < f64::EPSILON,
                "item[{i}] confidence {conf} is not rounded to 2dp (would be {rounded})"
            );
        }
    }

    let context_items = serde_json::to_string(&vec![serde_json::json!({
        "content": "ARR $2M",
        "quote": "ARR $2M",
        "source_episode": episode_id.clone()
    })])
    .unwrap();
    let context_pack = memory_mcp::tools::parsers::parse_context_items(&context_items)
        .expect("parse context_items for explain");
    let explain_request = ExplainRequest {
        context_pack,
        compact: false,
    };
    let explanation =
        ExplainCapability::explain(&mcp.service().build_context(), explain_request, None)
            .await
            .expect("explain");
    assert_eq!(explanation[0].source_episode, episode_id);

    // Verify rounding of decayed_confidence in explain response
    let explain_json = serde_json::to_value(&explanation).unwrap();
    if let Some(items) = explain_json.as_array() {
        for (i, item) in items.iter().enumerate() {
            if let Some(decayed) = item["decayed_confidence"].as_f64() {
                let rounded = (decayed * 100.0).round() / 100.0;
                assert!(
                    (decayed - rounded).abs() < f64::EPSILON,
                    "explain item[{i}] decayed_confidence {decayed} is not rounded to 2dp"
                );
            }
        }
    }

    let ingest_params2 = serde_json::json!({
        "source_type": "email",
        "source_id": "MSG-204",
        "content": "Follow-up: ARR $500k",
        "t_ref": "2026-01-11T00:00:00Z",
        "scope": "org"
    });
    let episode_id2 = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params2).unwrap()))
        .await
        .expect("ingest2")
        .0
        .result;

    let context_items_ids =
        serde_json::to_string(&vec![episode_id.clone(), episode_id2.clone()]).unwrap();
    let context_pack_ids = memory_mcp::tools::parsers::parse_context_items(&context_items_ids)
        .expect("parse context_items for explain ids");
    let explain_request_ids = ExplainRequest {
        context_pack: context_pack_ids,
        compact: false,
    };
    let explanation_ids =
        ExplainCapability::explain(&mcp.service().build_context(), explain_request_ids, None)
            .await
            .expect("explain ids");
    assert_eq!(explanation_ids[0].source_episode, episode_id);
    assert_eq!(explanation_ids[1].source_episode, episode_id2);
}

#[test]
fn test_mcp_extract_task_lifecycle_over_stdio() {
    let mut mcp = StdioMcpProcess::start();
    mcp.initialize();

    let ingest = mcp.request(
        2,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "source_type": "email",
                "source_id": "TASK-E2E-1",
                "content": "Alice Smith will deliver ARR $1M by Friday.",
                "t_ref": "2026-02-05T00:00:00Z",
                "scope": "org"
            }
        }),
    );
    let episode_id = ingest["result"]["structuredContent"]["result"]
        .as_str()
        .unwrap_or_else(|| panic!("missing episode id in ingest response: {ingest}"));
    let synchronous = mcp.request(
        3,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": episode_id}
        }),
    );
    let synchronous_result = synchronous["result"].clone();

    let created = mcp.request(
        4,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": episode_id},
            "task": {"ttl": 60_000}
        }),
    );
    let task_id = created["result"]["task"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing task id: {created}"))
        .to_string();
    let created_at = created["result"]["task"]["createdAt"]
        .as_str()
        .unwrap_or_else(|| panic!("missing task creation timestamp: {created}"))
        .to_string();
    assert_eq!(created["result"]["task"]["status"], "working");
    assert_eq!(
        created["result"]["_meta"]["io.modelcontextprotocol/related-task"]["taskId"], task_id,
        "task creation response must identify the related task: {created}"
    );

    let mut request_id = 5;
    let completed = mcp.wait_for_terminal_task(&mut request_id, &task_id);
    assert_eq!(completed["result"]["status"], "completed");
    assert_eq!(completed["result"]["createdAt"], created_at);

    let listed = mcp.request(request_id, "tasks/list", serde_json::json!({}));
    request_id += 1;
    assert!(
        listed["result"]["tasks"]
            .as_array()
            .is_some_and(|tasks| tasks.iter().any(|task| task["taskId"] == task_id)),
        "completed task must remain listable while its result is retained: {listed}"
    );
    let invalid_cursor = mcp.request(
        request_id,
        "tasks/list",
        serde_json::json!({"cursor": "missing-page"}),
    );
    request_id += 1;
    assert_eq!(
        invalid_cursor["error"]["code"], -32602,
        "unknown task cursors must use Invalid params: {invalid_cursor}"
    );

    let task_result = mcp.request(
        request_id,
        "tasks/result",
        serde_json::json!({"taskId": task_id}),
    );
    request_id += 1;
    assert_eq!(
        task_result["result"]["_meta"]["io.modelcontextprotocol/related-task"]["taskId"], task_id,
        "task payload must identify the related task: {task_result}"
    );
    assert_eq!(
        task_result["result"],
        task_payload_for_sync_result(synchronous_result, &task_id),
        "task and synchronous extract returned different payloads: {task_result}"
    );

    let repeated_result = mcp.request(
        request_id,
        "tasks/result",
        serde_json::json!({"taskId": task_id}),
    );
    request_id += 1;
    assert_eq!(
        repeated_result["result"], task_result["result"],
        "reading a task result must not consume it: {repeated_result}"
    );

    let unknown = mcp.request(
        request_id,
        "tasks/get",
        serde_json::json!({"taskId": "missing-task"}),
    );
    request_id += 1;
    assert_eq!(
        unknown["error"]["code"], -32602,
        "unknown task IDs must use Invalid params: {unknown}"
    );
    assert!(
        unknown["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("task not found: missing-task")),
        "unexpected unknown-task response: {unknown}"
    );

    for method in ["tasks/result", "tasks/cancel"] {
        let response = mcp.request(
            request_id,
            method,
            serde_json::json!({"taskId": "missing-task"}),
        );
        request_id += 1;
        assert_eq!(
            response["error"]["code"], -32602,
            "{method} must reject an unknown task as Invalid params: {response}"
        );
    }

    let forbidden_task = mcp.request(
        request_id,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {},
            "task": {"ttl": 60_000}
        }),
    );
    request_id += 1;
    assert_eq!(
        forbidden_task["error"]["code"], -32602,
        "non-task tools must reject task augmentation: {forbidden_task}"
    );

    let missing_arguments = serde_json::json!({"episode_id": "episode:missing-task-e2e"});
    let synchronous_failure = mcp.request(
        request_id,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": missing_arguments.clone()
        }),
    );
    request_id += 1;
    let asynchronous_failure = mcp.request(
        request_id,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": missing_arguments,
            "task": {"ttl": 60_000}
        }),
    );
    request_id += 1;
    let failed_task_id = asynchronous_failure["result"]["task"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing failed task id: {asynchronous_failure}"))
        .to_string();
    let failed_status = mcp.wait_for_terminal_task(&mut request_id, &failed_task_id);
    assert_eq!(failed_status["result"]["status"], "failed");
    let failed_result = mcp.request(
        request_id,
        "tasks/result",
        serde_json::json!({"taskId": failed_task_id}),
    );
    request_id += 1;
    if synchronous_failure["error"].is_object() {
        assert_eq!(
            failed_result["error"], synchronous_failure["error"],
            "task must preserve the original JSON-RPC error: {failed_result}"
        );
    } else {
        assert_eq!(
            failed_result["result"],
            task_payload_for_sync_result(synchronous_failure["result"].clone(), &failed_task_id),
            "task must preserve the original tool error payload: {failed_result}"
        );
    }

    let long_content = "Alice Smith met OpenAI in Moscow. ".repeat(25_000);
    let long_ingest = mcp.request(
        request_id,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "source_type": "document",
                "source_id": "TASK-E2E-CANCEL",
                "content": long_content,
                "t_ref": "2026-02-05T00:00:00Z",
                "scope": "org"
            }
        }),
    );
    request_id += 1;
    let long_episode_id = long_ingest["result"]["structuredContent"]["result"]
        .as_str()
        .unwrap_or_else(|| panic!("missing long episode id: {long_ingest}"));
    let cancellable = mcp.request(
        request_id,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": long_episode_id},
            "task": {"ttl": 60_000}
        }),
    );
    request_id += 1;
    let cancellable_id = cancellable["result"]["task"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing cancellable task id: {cancellable}"))
        .to_string();
    let cancellable_created_at = cancellable["result"]["task"]["createdAt"].clone();
    let cancelled = mcp.request(
        request_id,
        "tasks/cancel",
        serde_json::json!({"taskId": cancellable_id}),
    );
    request_id += 1;
    assert_eq!(
        cancelled["result"]["status"], "cancelled",
        "task cancellation failed: {cancelled}"
    );
    let cancelled_status = mcp.request(
        request_id,
        "tasks/get",
        serde_json::json!({"taskId": cancellable_id}),
    );
    assert_eq!(
        cancelled_status["result"]["status"], "cancelled",
        "cancelled is terminal and must remain observable: {cancelled_status}"
    );
    assert_eq!(
        cancelled_status["result"]["createdAt"],
        cancellable_created_at
    );
    request_id += 1;
    let cancelled_result = mcp.request(
        request_id,
        "tasks/result",
        serde_json::json!({"taskId": cancellable_id}),
    );
    assert_eq!(
        cancelled_result["error"]["code"], -32800,
        "cancelled task result must preserve cancellation: {cancelled_result}"
    );
}

#[tokio::test]
async fn test_mcp_full_flow_end_to_end() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "E2E-1",
        "content": "I will deliver ARR $1M by next week.",
        "t_ref": "2026-02-05T00:00:00Z",
        "scope": "org"
    });
    let episode_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest")
        .0
        .result;

    let extract_params = serde_json::json!({"episode_id": episode_id});
    let extraction = mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
        .expect("extract")
        .0
        .result;
    let facts = extraction.facts;
    assert!(facts.iter().any(|f| f.fact_type == "metric"));
    assert!(facts.iter().any(|f| f.fact_type == "promise"));

    let assemble_request: AssembleContextRequest = serde_json::from_value(serde_json::json!({
        "query": "ARR",
        "scope": "org",
        "as_of": Utc::now().to_rfc3339(),
        "budget": 5,
        "compact": false,
    }))
    .unwrap();
    let context = AssembleContextCapability::assemble_context(
        &mcp.service().build_context(),
        assemble_request,
    )
    .await
    .expect("assemble");
    assert!(!context.is_empty());

    let context_items = serde_json::to_string(&vec![serde_json::json!({"content": "ARR $1M","quote": "ARR $1M","source_episode": episode_id.clone()})]).unwrap();
    let context_pack = memory_mcp::tools::parsers::parse_context_items(&context_items)
        .expect("parse context_items for explain");
    let explain_request = ExplainRequest {
        context_pack,
        compact: false,
    };
    let explanation =
        ExplainCapability::explain(&mcp.service().build_context(), explain_request, None)
            .await
            .expect("explain");
    assert_eq!(explanation[0].source_episode, episode_id);

    let fact_id = context[0].fact_id.clone();
    let invalidate_params = serde_json::json!({"fact_id": fact_id, "reason": "superseded", "t_invalid": "2026-02-04T00:00:00Z"});
    let _ = mcp
        .invalidate(Parameters(
            serde_json::from_value(invalidate_params).unwrap(),
        ))
        .await
        .expect("invalidate");

    let assemble_request_after: AssembleContextRequest =
        serde_json::from_value(serde_json::json!({
            "query": "ARR",
            "scope": "org",
            "as_of": Utc::now().to_rfc3339(),
            "budget": 5,
            "compact": false,
        }))
        .unwrap();
    let context_after = AssembleContextCapability::assemble_context(
        &mcp.service().build_context(),
        assemble_request_after,
    )
    .await
    .expect("assemble");
    assert!(
        !context_after
            .iter()
            .any(|c| c.fact_id == context[0].fact_id)
    );
}

#[tokio::test]
async fn test_mcp_ingest_validation_error() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "",
        "source_id": "MSG-204",
        "content": "Missing source_type",
        "t_ref": "2026-01-10T00:00:00Z",
        "scope": "org"
    });

    let err = match mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
    {
        Ok(_) => panic!("expected ingest to fail validation"),
        Err(err) => err,
    };
    let message = err.to_string();
    assert!(message.contains("source_type"));
}

#[tokio::test]
async fn test_mcp_extract_no_input_returns_invalid_params_error() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let extract_params = serde_json::json!({
        "episode_id": "",
        "content": "",
        "text": null
    });

    let err = match mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
    {
        Ok(_) => panic!("expected extract to reject empty input"),
        Err(err) => err,
    };

    let message = err.to_string();
    assert!(
        message.contains("episode_id") && message.contains("payload"),
        "unexpected extract error: {message}"
    );
}

#[tokio::test]
async fn test_mcp_extract_rejects_ambiguous_episode_and_content_input() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let err = match mcp
        .extract(Parameters(
            serde_json::from_value(serde_json::json!({
                "episode_id": "episode:abc123",
                "content": "inline content"
            }))
            .unwrap(),
        ))
        .await
    {
        Ok(_) => panic!("expected extract to reject ambiguous inputs"),
        Err(err) => err,
    };

    let message = err.to_string();
    assert!(
        message.contains("exactly one") || message.contains("either"),
        "unexpected extract error: {message}"
    );
}

#[tokio::test]
async fn test_mcp_explain_rejects_legacy_object_aliases() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let context_items = serde_json::to_string(&vec![
        serde_json::json!({"content":"data","quote":"q","id":"task:abc","sourceType":"task"}),
    ])
    .unwrap();
    let explain_params = serde_json::json!({"context_items": context_items});
    let err = match mcp
        .explain(Parameters(serde_json::from_value(explain_params).unwrap()))
        .await
    {
        Ok(_) => panic!("expected explain to reject legacy aliases"),
        Err(err) => err,
    };

    let message = err.to_string();
    assert!(
        message.contains("source_episode") || message.contains("snake_case"),
        "unexpected explain error: {message}"
    );
}

#[tokio::test]
async fn test_mcp_explain_mixed_array() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let context_items = serde_json::to_string(&vec![
        serde_json::json!("episode:plain-id"),
        serde_json::json!({"content":"info","source_episode":"task:obj"}),
    ])
    .unwrap();
    let context_pack = memory_mcp::tools::parsers::parse_context_items(&context_items)
        .expect("parse context_items for explain mixed array");
    let explain_request = ExplainRequest {
        context_pack,
        compact: false,
    };
    let explanation =
        ExplainCapability::explain(&mcp.service().build_context(), explain_request, None)
            .await
            .expect("explain with mixed array should not fail");
    assert_eq!(explanation.len(), 2);
    assert_eq!(explanation[0].source_episode, "episode:plain-id");
    assert_eq!(explanation[1].source_episode, "task:obj");
    assert_eq!(explanation[1].content, "info");
}

#[tokio::test]
async fn test_mcp_explain_loads_episode_context() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "EXPLAIN-CTX-1",
        "content": "Customer confirmed ARR is now $3M and expects renewal next quarter.",
        "t_ref": "2026-02-15T08:30:00Z",
        "scope": "org"
    });
    let episode_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest explain context")
        .0
        .result;

    let context_items = serde_json::to_string(&vec![serde_json::json!({
        "content": "ARR is now $3M",
        "quote": "ARR is now $3M",
        "source_episode": episode_id.clone()
    })])
    .unwrap();

    let context_pack = memory_mcp::tools::parsers::parse_context_items(&context_items)
        .expect("parse context_items for explain loaded episode context");
    let explain_request = ExplainRequest {
        context_pack,
        compact: false,
    };
    let explanation =
        ExplainCapability::explain(&mcp.service().build_context(), explain_request, None)
            .await
            .expect("explain with loaded episode context");

    assert_eq!(explanation.len(), 1);
    assert_eq!(explanation[0].source_episode, episode_id);
    assert_eq!(explanation[0].scope.as_deref(), Some("org"));
    assert_eq!(
        explanation[0].t_ref.map(|dt| dt.to_rfc3339()),
        Some("2026-02-15T08:30:00+00:00".to_string())
    );
    assert!(explanation[0].t_ingested.is_some());
    assert_eq!(
        explanation[0].citation_context.as_deref(),
        Some("Customer confirmed ARR is now $3M and expects renewal next quarter.")
    );
    assert_eq!(
        explanation[0].provenance.get("source_episode"),
        Some(&serde_json::json!(explanation[0].source_episode.clone()))
    );
    assert_eq!(
        explanation[0].provenance.get("source_type"),
        Some(&serde_json::json!("email"))
    );
    assert_eq!(
        explanation[0].provenance.get("source_id"),
        Some(&serde_json::json!("EXPLAIN-CTX-1"))
    );
}

#[tokio::test]
async fn test_mcp_assemble_context_timeline_mode_passes_optional_fields() {
    let service = common::make_service().await;

    common::seed_fact_at(
        &service,
        "personal",
        "Atlas planning started",
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas budget increased",
        Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap(),
    )
    .await;

    let mcp = MemoryMcp::new(service);
    let params = serde_json::json!({
        "query": "atlas",
        "scope": "personal",
        "as_of": Utc::now().to_rfc3339(),
        "budget": 10,
        "view_mode": "timeline",
        "window_start": "2026-02-01T00:00:00Z",
        "window_end": "2026-02-28T23:59:59Z",
        "compact": false,
    });

    let assemble_request: AssembleContextRequest = serde_json::from_value(params).unwrap();
    let context = AssembleContextCapability::assemble_context(
        &mcp.service().build_context(),
        assemble_request,
    )
    .await
    .expect("assemble timeline");

    assert_eq!(context.len(), 1);
    assert_eq!(context[0].content, "Atlas budget increased");
}
