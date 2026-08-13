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
            .env("SURREALDB_NAMESPACE", "org")
            .env("SURREALDB_USERNAME", "root")
            .env("SURREALDB_PASSWORD", "root")
            .env("EMBEDDINGS_ENABLED", "false")
            .env("NER_EXTRACTOR", "anno")
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

    fn initialize(&mut self, task_capable: bool) {
        let client_capabilities = if task_capable {
            serde_json::json!({
                "extensions": {
                    "io.modelcontextprotocol/tasks": {}
                }
            })
        } else {
            serde_json::json!({})
        };
        let response = self.request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": client_capabilities,
                "clientInfo": {"name": "memory-mcp-e2e", "version": "1.0.0"}
            }),
        );
        let capabilities = &response["result"]["capabilities"];
        assert!(
            capabilities["extensions"]["io.modelcontextprotocol/tasks"].is_object(),
            "server must advertise the tasks extension: {response}"
        );
        self.notify("notifications/initialized", serde_json::json!({}));
    }

    fn wait_for_terminal_task(
        &mut self,
        request_id: &mut i64,
        task_id: &str,
        poll_interval_ms: u64,
    ) -> serde_json::Value {
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
                Some("working" | "input_required") => {
                    std::thread::sleep(Duration::from_millis(poll_interval_ms));
                }
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

#[tokio::test]
async fn test_mcp_tools_flow() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "MSG-203",
        "content": "I will finish it by Friday. ARR $2M",
        "t_ref": "2026-01-10T00:00:00Z"
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
        "t_ref": "2026-01-11T00:00:00Z"
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
    let mut synchronous_client = StdioMcpProcess::start();
    synchronous_client.initialize(false);
    let ingest = synchronous_client.request(
        2,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "source_type": "email",
                "source_id": "TASK-E2E-1",
                "content": "Alice Smith will deliver ARR $1M by Friday.",
                "t_ref": "2026-02-05T00:00:00Z"
            }
        }),
    );
    let episode_id = ingest["result"]["structuredContent"]["result"]
        .as_str()
        .unwrap_or_else(|| panic!("missing episode id in ingest response: {ingest}"))
        .to_string();
    let synchronous = synchronous_client.request(
        3,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": episode_id}
        }),
    );
    assert!(synchronous["result"]["structuredContent"].is_object());
    assert!(synchronous["result"]["taskId"].is_null());
    let synchronous_structured_content = synchronous["result"]["structuredContent"].clone();
    let synchronous_failure = synchronous_client.request(
        4,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": "episode:missing-task-e2e"}
        }),
    );
    assert!(synchronous_failure["error"].is_object());

    let mut task_client = StdioMcpProcess::start();
    task_client.initialize(true);
    let task_ingest = task_client.request(
        2,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "source_type": "email",
                "source_id": "TASK-E2E-1",
                "content": "Alice Smith will deliver ARR $1M by Friday.",
                "t_ref": "2026-02-05T00:00:00Z"
            }
        }),
    );
    let task_episode_id = task_ingest["result"]["structuredContent"]["result"]
        .as_str()
        .unwrap_or_else(|| panic!("missing task episode id: {task_ingest}"));
    assert!(task_ingest["result"]["taskId"].is_null());

    let created = task_client.request(
        3,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": task_episode_id}
        }),
    );
    let task_id = created["result"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing flattened task id: {created}"))
        .to_string();
    assert!(created["result"]["task"].is_null());
    assert_eq!(created["result"]["status"], "working");
    let created_at = created["result"]["createdAt"].clone();
    let poll_interval_ms = created["result"]["pollIntervalMs"]
        .as_u64()
        .unwrap_or(1_000);

    let mut request_id = 4;
    let completed = task_client.wait_for_terminal_task(&mut request_id, &task_id, poll_interval_ms);
    assert_eq!(completed["result"]["status"], "completed");
    assert_eq!(completed["result"]["createdAt"], created_at);
    assert_eq!(
        completed["result"]["result"]["structuredContent"],
        synchronous_structured_content
    );
    assert!(completed["result"]["error"].is_null());

    let update_ack = task_client.request(
        request_id,
        "tasks/update",
        serde_json::json!({"taskId": task_id, "inputResponses": {}}),
    );
    request_id += 1;
    assert!(update_ack["result"].is_object());

    let unknown = task_client.request(
        request_id,
        "tasks/get",
        serde_json::json!({"taskId": "missing-task"}),
    );
    request_id += 1;
    assert_eq!(unknown["error"]["code"], -32602);
    assert!(
        unknown["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("unknown task: missing-task")),
        "unexpected unknown-task response: {unknown}"
    );
    for method in ["tasks/update", "tasks/cancel"] {
        let params = if method == "tasks/update" {
            serde_json::json!({"taskId": "missing-task", "inputResponses": {}})
        } else {
            serde_json::json!({"taskId": "missing-task"})
        };
        let response = task_client.request(request_id, method, params);
        request_id += 1;
        assert_eq!(response["error"]["code"], -32602);
    }
    for method in ["tasks/list", "tasks/result"] {
        let response = task_client.request(request_id, method, serde_json::json!({}));
        request_id += 1;
        assert_eq!(response["error"]["code"], -32601);
    }

    let asynchronous_failure = task_client.request(
        request_id,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": "episode:missing-task-e2e"}
        }),
    );
    request_id += 1;
    let failed_task_id = asynchronous_failure["result"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing failed task id: {asynchronous_failure}"))
        .to_string();
    let failed_poll_interval_ms = asynchronous_failure["result"]["pollIntervalMs"]
        .as_u64()
        .unwrap_or(1_000);
    let failed_status = task_client.wait_for_terminal_task(
        &mut request_id,
        &failed_task_id,
        failed_poll_interval_ms,
    );
    assert_eq!(failed_status["result"]["status"], "failed");
    assert_eq!(
        failed_status["result"]["error"]["code"],
        synchronous_failure["error"]["code"]
    );
    assert!(failed_status["result"]["result"].is_null());

    let long_content = "Alice Smith met OpenAI in Moscow. ".repeat(25_000);
    let long_ingest = task_client.request(
        request_id,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "source_type": "document",
                "source_id": "TASK-E2E-CANCEL",
                "content": long_content,
                "t_ref": "2026-02-05T00:00:00Z"
            }
        }),
    );
    request_id += 1;
    let long_episode_id = long_ingest["result"]["structuredContent"]["result"]
        .as_str()
        .unwrap_or_else(|| panic!("missing long episode id: {long_ingest}"));
    let cancellable = task_client.request(
        request_id,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": long_episode_id}
        }),
    );
    request_id += 1;
    let cancellable_id = cancellable["result"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing cancellable task id: {cancellable}"))
        .to_string();
    let cancellable_created_at = cancellable["result"]["createdAt"].clone();
    let cancellable_poll_interval_ms = cancellable["result"]["pollIntervalMs"]
        .as_u64()
        .unwrap_or(1_000);
    let cancelled = task_client.request(
        request_id,
        "tasks/cancel",
        serde_json::json!({"taskId": cancellable_id}),
    );
    request_id += 1;
    assert!(cancelled["result"].is_object());
    let cancelled_status = task_client.wait_for_terminal_task(
        &mut request_id,
        &cancellable_id,
        cancellable_poll_interval_ms,
    );
    assert!(matches!(
        cancelled_status["result"]["status"].as_str(),
        Some("completed" | "failed" | "cancelled")
    ));
    assert_eq!(
        cancelled_status["result"]["createdAt"],
        cancellable_created_at
    );
}

#[cfg(feature = "mcp-apps")]
#[test]
fn test_mcp_resources_read_over_stdio() {
    let mut client = StdioMcpProcess::start();
    client.initialize(false);

    let listed = client.request(2, "resources/list", serde_json::json!({}));
    let resources = listed["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("missing resource catalog: {listed}"));
    assert!(
        resources
            .iter()
            .any(|resource| { resource["uri"].as_str() == Some("ui://memory/apps") })
    );

    let response = client.request(
        3,
        "resources/read",
        serde_json::json!({"uri": "ui://memory/apps"}),
    );
    assert!(
        response["error"].is_null(),
        "resources/read failed: {response}"
    );
    let contents = response["result"]["contents"]
        .as_array()
        .unwrap_or_else(|| panic!("missing resource contents: {response}"));
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["uri"], "ui://memory/apps");
    assert_eq!(contents[0]["mimeType"], "application/json");
    let payload: serde_json::Value = serde_json::from_str(
        contents[0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("resource content is not text: {response}")),
    )
    .unwrap_or_else(|error| panic!("resource content is not JSON: {error}: {response}"));
    assert!(
        payload["apps"]
            .as_array()
            .is_some_and(|apps| !apps.is_empty())
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
        "t_ref": "2026-02-05T00:00:00Z"
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
        "t_ref": "2026-01-10T00:00:00Z"
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
        "t_ref": "2026-02-15T08:30:00Z"
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

/// Regression test for the user-reported bug: a bare hex string (no
/// `episode:` prefix) passed as `episode_id` to `extract` used to return a
/// misleading `Episode not found: <hex>` error because the query builder's
/// safe-noop turned the malformed id into `SELECT * FROM none WHERE false`
/// and the DB reported it as missing.
///
/// After the fix (Tasks 1-3 in this plan):
///   - `validate_record_id` runs at every record-id entry point and rejects
///     bare hex with a `Validation` error that names the canonical
///     `<table>:<id>` form and echoes the bad input.
///   - The MCP `extract` tool surfaces this as `INVALID_PARAMS` (the same
///     error code as before) but with a message that explains the input
///     shape, not a fake "not found".
#[tokio::test]
async fn extract_with_bare_hex_episode_id_returns_validation_error_not_not_found() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    // 1. Ingest a real episode and get back the canonical id.
    let ingest_params = serde_json::json!({
        "source_type": "ad-hoc",
        "source_id": "reproducer:bare-hex-bug",
        "content": "Test content: meeting notes about EPS reduction decision.",
        "t_ref": "2026-07-31T18:00:00Z"
    });
    let canonical_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest")
        .0
        .result;
    assert!(
        canonical_id.starts_with("episode:"),
        "expected canonical 'episode:<hex>' id, got {canonical_id}"
    );

    // 2. Strip the prefix the way a broken client might.
    let bare_hex = canonical_id.trim_start_matches("episode:").to_string();

    // 3. Call extract with the bare hex. Pre-fix: misleading "Episode not found".
    //    Post-fix: a clear Validation error that names the expected form.
    let extract_params = serde_json::json!({ "episode_id": bare_hex });
    let err = match mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
    {
        Ok(_) => panic!(
            "extract with bare hex must fail validation; got Ok. \
             The bug regression is back: bare hex '{bare_hex}' should be rejected."
        ),
        Err(err) => err,
    };
    let message = err.to_string();
    assert!(
        message.contains("'<table>:<id>'") || message.contains("no ':' separator"),
        "expected validation message to name the canonical form, got: {message}"
    );
    assert!(
        !message.starts_with("Episode not found"),
        "bug regression: extract still returns misleading 'Episode not found': {message}"
    );
    assert!(
        message.contains(&bare_hex),
        "validation message should echo the bad input '{bare_hex}', got: {message}"
    );

    // 4. Control: extract with the correct prefixed id must succeed.
    let ok_params = serde_json::json!({ "episode_id": canonical_id });
    mcp.extract(Parameters(serde_json::from_value(ok_params).unwrap()))
        .await
        .expect("well-formed episode_id must succeed");
}
