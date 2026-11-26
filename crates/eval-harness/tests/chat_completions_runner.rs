use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Instant;

use eval_harness::EvalError;
use eval_harness::paired::TraceEventKind;
use eval_harness::paired_agent::chat_completions::{
    ChatCompletionBudgets, ChatCompletionsAgent, ChatCompletionsConfig, ChatCompletionsSampling,
    ChatCompletionsToolProfile, MemoryToolSession,
};
use serde_json::{Value, json};

struct FakeMemorySession {
    calls: Vec<(String, Value)>,
    schemas: Value,
    reply_override: Option<Value>,
}

impl FakeMemorySession {
    fn read_only() -> Self {
        Self {
            calls: Vec::new(),
            reply_override: None,
            schemas: json!([{
                "name": "assemble_context",
                "description": "Read relevant memory",
                "inputSchema": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}, "budget": {"type": "integer"}},
                    "required": ["query", "budget"],
                    "additionalProperties": false
                }
            }]),
        }
    }

    fn with_reply(reply: Value) -> Self {
        Self {
            reply_override: Some(reply),
            ..Self::read_only()
        }
    }
}

impl MemoryToolSession for FakeMemorySession {
    fn tool_schemas(&self) -> &Value {
        &self.schemas
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, EvalError> {
        let model_query = arguments["query"] == "model query";
        self.calls.push((name.to_owned(), arguments));
        if let Some(reply) = &self.reply_override {
            return Ok(reply.clone());
        }
        if model_query {
            return Ok(json!({"result":{"structuredContent":{"result":[]}}}));
        }
        Ok(json!({
            "result": {
                "structuredContent": {
                    "result": [{"source_episode": "episode:cedar", "statement": "Cabinet-28"}]
                }
            }
        }))
    }
}

struct MockChatServer {
    endpoint: String,
    requests: thread::JoinHandle<Vec<Value>>,
}

impl MockChatServer {
    fn with_responses(responses: Vec<Value>) -> Self {
        Self::with_http_responses(
            responses
                .into_iter()
                .map(|response| (200, response))
                .collect(),
        )
    }

    fn with_http_responses(responses: Vec<(u16, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock model endpoint");
        let address = listener.local_addr().expect("read mock endpoint address");
        let requests = thread::spawn(move || serve_chat_requests(listener, responses));
        Self {
            endpoint: format!("http://{address}/v1"),
            requests,
        }
    }

    fn with_truncated_body(advertised_length: usize, body: &[u8]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind truncated-body server");
        let address = listener.local_addr().expect("read mock endpoint address");
        let body = body.to_vec();
        let requests = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept model request");
            let request = read_json_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {advertised_length}\r\nConnection: close\r\n\r\n"
            )
            .expect("write truncated response headers");
            stream
                .write_all(&body)
                .expect("write partial response body");
            vec![request]
        });
        Self {
            endpoint: format!("http://{address}/v1"),
            requests,
        }
    }

    fn requests(self) -> Vec<Value> {
        self.requests.join().expect("mock model server exits")
    }
}

fn serve_chat_requests(listener: TcpListener, responses: Vec<(u16, Value)>) -> Vec<Value> {
    let mut requests = Vec::new();
    for (status, response) in responses {
        let (mut stream, _) = listener.accept().expect("accept model request");
        requests.push(read_json_request(&mut stream));
        let body = serde_json::to_vec(&response).expect("serialize model response");
        write!(
            stream,
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("write mock response headers");
        stream.write_all(&body).expect("write mock response body");
    }
    requests
}

fn redirecting_server() -> (String, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect test server");
    let address = listener.local_addr().expect("read redirect server address");
    let endpoint = format!("http://{address}/v1");
    let task_endpoint = endpoint.clone();
    let worker = thread::spawn(move || {
        let (mut first, _) = listener.accept().expect("accept initial request");
        let _ = read_json_request(&mut first);
        write!(
            first,
            "HTTP/1.1 302 Found\r\nLocation: {task_endpoint}/chat/completions\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .expect("write redirect response");
        listener
            .set_nonblocking(true)
            .expect("observe redirected follow-up without blocking");
        let deadline = Instant::now() + std::time::Duration::from_millis(250);
        let mut follow_up_count = 0;
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut redirected, _)) => {
                    follow_up_count += 1;
                    let _ = read_json_request(&mut redirected);
                    let body = serde_json::to_vec(&model_response(
                        json!({
                            "role":"assistant",
                            "content":"{\"answer\":\"Cabinet-28\",\"citations\":[],\"needs_clarification\":false}"
                        }),
                        5,
                        2,
                    ))
                    .expect("serialize redirect target response");
                    write!(
                        redirected,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .expect("write redirect target headers");
                    redirected
                        .write_all(&body)
                        .expect("write redirect target body");
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::yield_now();
                }
                Err(error) => panic!("accept redirect follow-up: {error}"),
            }
        }
        follow_up_count
    });
    (endpoint, worker)
}

fn read_json_request(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut one = [0_u8; 1];
    let mut header_end = None;
    while stream.read_exact(&mut one).is_ok() {
        bytes.push(one[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            header_end = Some(bytes.len());
            break;
        }
    }
    let header_end = header_end.expect("request headers end");
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .expect("content-length header");
    let mut body = vec![0; content_length];
    stream.read_exact(&mut body).expect("read request body");
    serde_json::from_slice(&body).expect("parse model request JSON")
}

fn model_response(message: Value, prompt_tokens: u64, completion_tokens: u64) -> Value {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "model": "test-model",
        "choices": [{"index": 0, "message": message, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens}
    })
}

fn capture_read_reply(reply: Value) -> Value {
    struct ReplySession(FakeMemorySession, Option<Value>);

    impl MemoryToolSession for ReplySession {
        fn tool_schemas(&self) -> &Value {
            self.0.tool_schemas()
        }

        fn call_tool(&mut self, _: &str, _: Value) -> Result<Value, EvalError> {
            Ok(self.1.take().expect("one registered read"))
        }
    }

    let server = MockChatServer::with_responses(vec![
        model_response(
            json!({
                "role": "assistant", "content": null, "tool_calls": [{
                    "id": "read-1", "type": "function",
                    "function": {"name": "assemble_context",
                        "arguments": r#"{"query":"Where is Cedar?","budget":1}"#}
                }]
            }),
            40,
            12,
        ),
        model_response(
            json!({
                "role": "assistant",
                "content": r#"{"answer":"unknown","citations":[],"needs_clarification":true}"#
            }),
            65,
            18,
        ),
    ]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .unwrap();
    let mut memory = ReplySession(FakeMemorySession::read_only(), Some(reply));
    let observation = agent
        .run_phase(
            &mut memory,
            "Where is Cedar?",
            "Use memory.",
            "Return JSON.",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["tool_choice"], "auto");
    assert_eq!(requests[1]["tool_choice"], "auto");
    serde_json::to_value(observation).unwrap()
}

#[test]
fn fixed_reply_is_delivered_to_the_original_tool_call() {
    use eval_harness::paired_agent::chat_completions::{RecallMemorySession, RecallReadMode};

    let server = MockChatServer::with_responses(vec![
        model_response(
            json!({"role":"assistant","content":null,"tool_calls":[{
                "id":"read-1","type":"function","function":{"name":"assemble_context",
                "arguments":r#"{"query":"model query","budget":1}"#}
            }]}),
            40,
            12,
        ),
        model_response(
            json!({"role":"assistant",
                "content":r#"{"answer":"Cabinet-28","citations":["episode:cedar"]}"#
            }),
            65,
            18,
        ),
    ]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .unwrap();
    let mut product = FakeMemorySession::read_only();
    let fixed = json!({"query":"registered query","budget":1});
    let mut memory =
        RecallMemorySession::new(&mut product, RecallReadMode::FixedReply(fixed.clone())).unwrap();
    assert_eq!(
        memory.tool_schemas(),
        FakeMemorySession::read_only().tool_schemas()
    );
    agent
        .run_phase(
            &mut memory,
            "Where is Cedar?",
            "Use memory.",
            "Return JSON.",
            ChatCompletionsToolProfile::RecallOnly,
            ChatCompletionBudgets {
                tool_calls: 1,
                ..budgets()
            },
        )
        .unwrap();

    assert_eq!(product.calls, [("assemble_context".into(), fixed)]);
    let requests = server.requests();
    assert_eq!(requests[0]["tool_choice"], "auto");
    assert!(requests[1].get("tool_choice").is_none());
    assert!(requests[1].get("tools").is_none());
    assert!(requests[1].get("parallel_tool_calls").is_none());
    let tool = requests[1]["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(tool["tool_call_id"], "read-1");
    assert_eq!(
        serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap(),
        json!({"result":{"structuredContent":{"result":[
            {"source_episode":"episode:cedar","statement":"Cabinet-28"}
        ]}}})
    );
    assert_eq!(
        requests[0]["tools"][0]["function"]["name"],
        "assemble_context"
    );
}

#[test]
fn fixed_read_registration_accepts_evidence_action_hint() {
    use eval_harness::paired_agent::chat_completions_backend::FixedReadInput;

    let parsed = serde_json::from_value::<FixedReadInput>(json!({
        "key": {
            "stratum_id":"ordinary",
            "family_id":"family-a",
            "case_id":"case-a",
            "repetition":1,
            "incumbent_first":true
        },
        "arm":"candidate",
        "arguments":{"query":"registered query","budget":1},
        "expected_source_events":[],
        "expected_completeness":null,
        "presentation":"evidence_action_hint"
    }));

    if let Err(error) = parsed {
        panic!("presentation registration was rejected: {error}");
    }
}

#[test]
fn evidence_action_hint_reaches_the_model_with_the_same_returned_source() {
    use eval_harness::paired_agent::chat_completions::{RecallMemorySession, RecallReadMode};

    let public_reply = json!({
        "result": {"structuredContent": {
            "context": {"completeness":"complete"},
            "guidance":"Original public guidance.",
            "result": [{"source_episode":"episode:cedar","content":"Cabinet-28","evidence_status":"current"}]
        }}
    });
    let server = MockChatServer::with_responses(vec![
        model_response(
            json!({"role":"assistant","content":null,"tool_calls":[{
                "id":"read-1","type":"function","function":{"name":"assemble_context",
                "arguments":r#"{"query":"model query","budget":1}"#}
            }]}),
            40,
            12,
        ),
        model_response(
            json!({"role":"assistant","content":r#"{"answer":"Cabinet-28","citations":["episode:cedar"]}"#}),
            65,
            18,
        ),
    ]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .unwrap();
    let mut product = FakeMemorySession::with_reply(public_reply);
    let fixed = json!({"query":"registered query","budget":1});
    let mut memory = RecallMemorySession::new(
        &mut product,
        RecallReadMode::FixedReplyWithEvidenceActionHint(fixed.clone()),
    )
    .unwrap();

    agent
        .run_phase(
            &mut memory,
            "Where is Cedar?",
            "Use memory.",
            "Return JSON.",
            ChatCompletionsToolProfile::RecallOnly,
            ChatCompletionBudgets {
                tool_calls: 1,
                ..budgets()
            },
        )
        .unwrap();

    let requests = server.requests();
    let tool = requests[1]["messages"].as_array().unwrap().last().unwrap();
    let delivered: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
    let content = &delivered["result"]["structuredContent"];
    assert_eq!(content["result"][0]["source_episode"], "episode:cedar");
    assert_eq!(
        content["agent_action_hint"]["action"],
        "answer_with_sources"
    );
    assert_eq!(
        content["agent_action_hint"]["source_episode_ids"],
        json!(["episode:cedar"])
    );
    assert!(
        content["guidance"]
            .as_str()
            .unwrap()
            .starts_with("Original public guidance.\nEvaluation-only action hint (not evidence)")
    );
    assert!(
        content["agent_action_hint"]["instruction"]
            .as_str()
            .unwrap()
            .contains("only when the returned evidence directly supports the question")
    );
    assert_eq!(product.calls, [("assemble_context".into(), fixed)]);
}

#[test]
fn evidence_action_hint_marks_a_typed_conflict_for_clarification() {
    use eval_harness::paired_agent::chat_completions::{RecallMemorySession, RecallReadMode};

    let public_reply = json!({
        "result": {"structuredContent": {
            "context": {"completeness":"partial"},
            "result": [{
                "source_episode":"episode:cedar-current",
                "evidence_status":"conflicting",
                "reconciliation":{"relations":[{"outcome":"contradiction"}]}
            }]
        }}
    });
    let mut product = FakeMemorySession::with_reply(public_reply);
    let mut memory = RecallMemorySession::new(
        &mut product,
        RecallReadMode::FixedReplyWithEvidenceActionHint(json!({
            "query":"registered query","budget":1
        })),
    )
    .unwrap();

    let delivered = memory
        .call_tool(
            "assemble_context",
            json!({"query":"model query","budget":1}),
        )
        .unwrap();

    let hint = &delivered["result"]["structuredContent"]["agent_action_hint"];
    assert_eq!(hint["action"], "clarify_conflict");
    assert_eq!(hint["source_episode_ids"], json!(["episode:cedar-current"]));
    assert!(
        hint["instruction"]
            .as_str()
            .unwrap()
            .contains("do not choose a side")
    );
}

#[test]
fn evidence_action_hint_fails_closed_when_any_item_status_is_unknown_or_missing() {
    use eval_harness::paired_agent::chat_completions::{RecallMemorySession, RecallReadMode};

    let public_reply = json!({
        "result": {"structuredContent": {
            "context": {"completeness":"partial"},
            "result": [
                {"source_episode":"episode:known","evidence_status":"current"},
                {"source_episode":"episode:unknown","evidence_status":"unknown"},
                {"source_episode":"episode:missing-status"}
            ]
        }}
    });
    let mut product = FakeMemorySession::with_reply(public_reply);
    let mut memory = RecallMemorySession::new(
        &mut product,
        RecallReadMode::FixedReplyWithEvidenceActionHint(json!({
            "query":"registered query","budget":1
        })),
    )
    .unwrap();

    let delivered = memory
        .call_tool(
            "assemble_context",
            json!({"query":"model query","budget":1}),
        )
        .unwrap();

    let hint = &delivered["result"]["structuredContent"]["agent_action_hint"];
    assert_eq!(hint["action"], "verify_or_ask");
    assert_eq!(
        hint["source_episode_ids"],
        json!(["episode:known", "episode:missing-status", "episode:unknown"])
    );
    assert!(
        hint["instruction"]
            .as_str()
            .unwrap()
            .contains("do not prove absence")
    );
}

#[test]
fn evidence_action_hint_preserves_an_opaque_public_reply() {
    use eval_harness::paired_agent::chat_completions::{RecallMemorySession, RecallReadMode};

    let public_reply = json!({
        "result": {"content":[{"type":"text","text":"OPAQUE_REPLY_SENTINEL"}]}
    });
    let mut product = FakeMemorySession::with_reply(public_reply.clone());
    let mut memory = RecallMemorySession::new(
        &mut product,
        RecallReadMode::FixedReplyWithEvidenceActionHint(json!({
            "query":"registered query","budget":1
        })),
    )
    .unwrap();

    let delivered = memory
        .call_tool(
            "assemble_context",
            json!({"query":"model query","budget":1}),
        )
        .unwrap();

    assert_eq!(delivered, public_reply);
    assert_eq!(product.calls.len(), 1);
}

#[test]
fn evidence_action_hint_on_an_empty_partial_read_does_not_claim_absence() {
    use eval_harness::paired_agent::chat_completions::{RecallMemorySession, RecallReadMode};

    let public_reply = json!({
        "result": {"structuredContent": {
            "context": {"completeness":"partial"},
            "guidance":"The historical context is partial.",
            "result": []
        }}
    });
    let mut product = FakeMemorySession::with_reply(public_reply);
    let mut memory = RecallMemorySession::new(
        &mut product,
        RecallReadMode::FixedReplyWithEvidenceActionHint(json!({
            "query":"registered query","budget":1
        })),
    )
    .unwrap();

    let delivered = memory
        .call_tool(
            "assemble_context",
            json!({"query":"model query","budget":1}),
        )
        .unwrap();

    let hint = &delivered["result"]["structuredContent"]["agent_action_hint"];
    assert_eq!(hint["action"], "verify_or_ask");
    assert_eq!(hint["source_episode_ids"], json!([]));
    assert!(
        hint["instruction"]
            .as_str()
            .unwrap()
            .contains("does not prove absence")
    );
}

#[test]
fn jsonrpc_read_error_remains_a_failed_read() {
    let captured =
        capture_read_reply(json!({"error":{"code":-32603,"message":"public synthetic failure"}}));
    assert_eq!(captured["memory_reads"][0]["outcome"], "failed");
}

#[test]
fn read_error_does_not_become_empty_memory() {
    let captured = capture_read_reply(json!({"result":{"isError":true}}));
    let read = &captured["memory_reads"][0];
    assert_eq!(read["outcome"], "failed");
    assert!(read["item_count"].is_null());
    let trace = captured["trace"].as_array().unwrap();
    let request = &trace[read["request_sequence"].as_u64().unwrap() as usize - 1];
    let response = &trace[read["response_sequence"].as_u64().unwrap() as usize - 1];
    assert_eq!(request["kind"], "memory_read");
    assert_eq!(response["kind"], "tool_error");
    assert_eq!(request["tool_name"], "assemble_context");
    assert_eq!(response["call_id"], request["call_id"]);
}

#[test]
fn text_only_loopback_reply_has_unknown_read_outcome() {
    let captured = capture_read_reply(json!({"result":{"content":[
        {"type":"text","text":"UNRECOGNIZED_REPLY_SENTINEL"}
    ]}}));
    assert_eq!(captured["memory_reads"][0]["outcome"], "unknown");
    assert!(captured["memory_reads"][0]["item_count"].is_null());
    assert!(!captured.to_string().contains("UNRECOGNIZED_REPLY_SENTINEL"));
}

#[test]
fn loopback_read_normalizes_only_the_matched_structured_reply() {
    use sha2::Digest;
    let captured = capture_read_reply(json!({"result":{"structuredContent":{
        "result":[{
            "source_episode":"episode:cedar",
            "statement":"PRIVATE_STATEMENT_SENTINEL",
            "evidence_status":"conflicting",
            "reconciliation":{"relations":[{"outcome":"contradiction","role":"conflict"}]}
        }],
        "context":{"completeness":"partial", "snapshot":{
            "valid_at":"2026-05-01T11:00:00+01:00","known_revision":8
        }}
    }}}));
    let evidence = &captured["memory_reads"][0]["evidence"];
    let digest = hex::encode(sha2::Sha256::digest(br#"["2026-05-01T10:00:00Z",8]"#));
    assert_eq!(evidence["returned_cutoff_sha256"], digest);
    assert_eq!(evidence["completeness"], "partial");
    assert_eq!(evidence["source_episode_ids"], json!(["episode:cedar"]));
    assert_eq!(
        evidence["claim_annotations"],
        json!([{
            "source_event_id":"episode:cedar","evidence_status":"conflicting",
            "relations":[{"outcome":"contradiction","role":"conflict"}]
        }])
    );
    assert!(!captured.to_string().contains("PRIVATE_STATEMENT_SENTINEL"));
}

#[test]
fn empty_read_reports_zero_items() {
    let captured = capture_read_reply(json!({"result":{"structuredContent":{"result":[]}}}));
    let read = &captured["memory_reads"][0];
    assert_eq!(read["outcome"], "empty");
    assert_eq!(read["item_count"], 0);
}

#[test]
fn chat_completions_agent_executes_an_allowed_mcp_tool_and_records_only_receipt_metadata() {
    let final_answer =
        r#"{"answer":"Cabinet-28","citations":["episode:cedar"],"needs_clarification":false}"#;
    let server = MockChatServer::with_responses(vec![
        model_response(
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": {
                        "name": "assemble_context",
                        "arguments": "{\"query\":\"Where is the Cedar ledger?\",\"budget\":1}"
                    }
                }]
            }),
            40,
            12,
        ),
        model_response(
            json!({"role": "assistant", "content": final_answer}),
            65,
            18,
        ),
    ]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let budgets = ChatCompletionBudgets {
        context_tokens: 256,
        tool_calls: 2,
        turns: 3,
        output_tokens: 128,
        wall_time: std::time::Duration::from_secs(5),
    };
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "For Cedar, where is the calibration ledger?",
            "Use registered memory tools and cite exact source episode IDs.",
            "Return JSON with answer, citations, and needs_clarification.",
            ChatCompletionsToolProfile::RecallOnly,
            budgets,
        )
        .expect("local model completes one memory read and answer");

    assert_eq!(
        memory.calls,
        [(
            "assemble_context".to_owned(),
            json!({"query":"Where is the Cedar ledger?","budget":1})
        )]
    );
    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    assert_eq!(observation.final_answer.as_deref(), Some(final_answer));
    let capture = serde_json::to_value(&observation).unwrap();
    assert_eq!(capture["memory_reads"][0]["outcome"], "nonempty");
    assert!(capture["memory_reads"][0]["elapsed_ms"].as_u64().is_some());
    assert_eq!(capture["memory_reads"][0]["item_count"], 1);
    assert_eq!(observation.memory_source_episode_ids, ["episode:cedar"]);
    assert_eq!(observation.turns_used, 2);
    assert_eq!(observation.tool_calls, 1);
    assert_eq!(observation.input_tokens, 105);
    assert_eq!(observation.output_tokens, 30);
    assert_eq!(
        observation
            .trace
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7]
    );
    assert!(observation.trace.iter().any(|event| {
        event.kind == TraceEventKind::MemoryRead
            && event.tool_name.as_deref() == Some("assemble_context")
    }));
    let serialized_trace = serde_json::to_string(&observation.trace).expect("serialize trace");
    assert!(!serialized_trace.contains("Cabinet-28"));
    assert!(!serialized_trace.contains("Where is the Cedar ledger?"));

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], "test-model");
    assert_eq!(requests[0]["temperature"], 0.0);
    assert!(requests[0].get("top_p").is_none());
    assert!(requests[0].get("top_k").is_none());
    assert!(requests[0].get("min_p").is_none());
    assert!(requests[0].get("presence_penalty").is_none());
    assert!(requests[0].get("repeat_penalty").is_none());
    assert_eq!(
        requests[0]["tools"][0]["function"]["name"],
        "assemble_context"
    );
    assert_eq!(requests[1]["max_tokens"], 116);
    assert_eq!(requests[1]["messages"][2]["role"], "assistant");
    assert_eq!(requests[1]["messages"][3]["role"], "tool");
}

#[test]
fn chat_completions_agent_sends_one_combined_system_message() {
    let final_answer = r#"{"answer":"Cabinet-28","citations":[],"needs_clarification":false}"#;
    let server = MockChatServer::with_responses(vec![model_response(
        json!({"role":"assistant","content":final_answer}),
        40,
        12,
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    agent
        .run_phase(
            &mut memory,
            "Where is the Cedar ledger?",
            "Use registered memory tools and cite exact source episode IDs.",
            "Return JSON with answer, citations, and needs_clarification.",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("fake model returns one valid answer");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let messages = requests[0]["messages"]
        .as_array()
        .expect("Chat Completions sends a message array");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(
        messages[0]["content"],
        "Use registered memory tools and cite exact source episode IDs.\n\nReturn JSON with answer, citations, and needs_clarification."
    );
    assert_eq!(messages[1]["role"], "user");
}

#[test]
fn chat_completions_agent_sends_registered_response_format_on_tool_and_final_turns() {
    let response_format = json!({
        "type": "json_schema",
        "json_schema": {
            "name": "memory_recall_answer",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "answer": {"type": "string"},
                    "citations": {"type": "array", "items": {"type": "string"}},
                    "needs_clarification": {"type": "boolean"}
                },
                "required": ["answer", "citations", "needs_clarification"],
                "additionalProperties": false
            }
        }
    });
    let final_answer =
        r#"{"answer":"Cabinet-28","citations":["episode:cedar"],"needs_clarification":false}"#;
    let server = MockChatServer::with_responses(vec![
        model_response(
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call-structured-answer",
                    "type": "function",
                    "function": {
                        "name": "assemble_context",
                        "arguments": "{\"query\":\"Where is the Cedar ledger?\",\"budget\":1}"
                    }
                }]
            }),
            40,
            12,
        ),
        model_response(
            json!({"role": "assistant", "content": final_answer}),
            65,
            18,
        ),
    ]);
    let agent = ChatCompletionsAgent::new(
        ChatCompletionsConfig::new(server.endpoint.clone(), "test-model")
            .with_response_format(Some(response_format.clone())),
    )
    .expect("JSON schema response format is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "Where is the Cedar ledger?",
            "Use registered memory tools and cite exact source episode IDs.",
            "Return JSON with answer, citations, and needs_clarification.",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("model completes tool call and structured final answer");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    assert_eq!(memory.calls.len(), 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["response_format"], response_format);
    assert_eq!(requests[1]["response_format"], response_format);
}

#[test]
fn chat_completions_agent_sends_registered_provider_parameters_on_every_turn() {
    let request_parameters = json!({
        "chat_template_kwargs": {"enable_thinking": false},
        "reasoning_effort": "low"
    });
    let server = MockChatServer::with_responses(vec![
        model_response(
            json!({
                "role":"assistant",
                "content":null,
                "tool_calls":[{
                    "id":"call-provider-parameters",
                    "type":"function",
                    "function":{
                        "name":"assemble_context",
                        "arguments":"{\"query\":\"Where is the Cedar ledger?\",\"budget\":1}"
                    }
                }]
            }),
            40,
            12,
        ),
        model_response(json!({"role":"assistant","content":"Cabinet-28"}), 25, 7),
    ]);
    let agent = ChatCompletionsAgent::new(
        ChatCompletionsConfig::new(server.endpoint.clone(), "test-model")
            .with_request_parameters(Some(request_parameters.clone())),
    )
    .expect("provider parameters are valid JSON object");
    let mut memory = FakeMemorySession::read_only();

    agent
        .run_phase(
            &mut memory,
            "Where is the Cedar ledger?",
            "Use registered memory tools and cite exact source episode IDs.",
            "Return JSON with answer, citations, and needs_clarification.",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("fake model completes a tool call and final answer");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["chat_template_kwargs"],
        request_parameters["chat_template_kwargs"]
    );
    assert_eq!(
        requests[1]["chat_template_kwargs"],
        request_parameters["chat_template_kwargs"]
    );
    assert_eq!(
        requests[0]["reasoning_effort"],
        request_parameters["reasoning_effort"]
    );
    assert_eq!(
        requests[1]["reasoning_effort"],
        request_parameters["reasoning_effort"]
    );
}

#[test]
fn chat_completions_agent_preserves_optional_reasoning_for_tool_followup_without_recording_it() {
    let private_reasoning = "private assistant reasoning that must stay out of receipts";
    let final_answer =
        r#"{"answer":"Cabinet-28","citations":["episode:cedar"],"needs_clarification":false}"#;
    let server = MockChatServer::with_responses(vec![
        model_response(
            json!({
                "role":"assistant",
                "content":null,
                "reasoning_content":private_reasoning,
                "tool_calls":[{
                    "id":"call-reasoning",
                    "type":"function",
                    "function":{
                        "name":"assemble_context",
                        "arguments":"{\"query\":\"Where is the Cedar ledger?\",\"budget\":1}"
                    }
                }]
            }),
            40,
            12,
        ),
        model_response(json!({"role":"assistant","content":final_answer}), 65, 18),
    ]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "For Cedar, where is the calibration ledger?",
            "Use registered memory tools and cite exact source episode IDs.",
            "Return JSON with answer, citations, and needs_clarification.",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("local model completes one memory read and answer");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    assert!(
        !serde_json::to_string(&observation.trace)
            .expect("serialize content-free trace")
            .contains(private_reasoning)
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["messages"][2]["role"], "assistant");
    assert_eq!(
        requests[1]["messages"][2]["reasoning_content"],
        private_reasoning
    );
    assert_eq!(requests[1]["messages"][3]["role"], "tool");
}

#[test]
fn chat_completions_agent_sends_registered_sampling_parameters() {
    let final_answer = r#"{"answer":"Cabinet-28","citations":[],"needs_clarification":false}"#;
    let server = MockChatServer::with_responses(vec![model_response(
        json!({"role":"assistant","content":final_answer}),
        40,
        12,
    )]);
    let sampling = ChatCompletionsSampling {
        temperature: 1.0,
        top_p: Some(0.95),
        top_k: Some(20),
        min_p: Some(0.0),
        presence_penalty: Some(0.0),
        repeat_penalty: Some(1.0),
    };
    let agent = ChatCompletionsAgent::new(
        ChatCompletionsConfig::new(server.endpoint.clone(), "test-model").with_sampling(sampling),
    )
    .expect("loopback endpoint and sampling parameters are valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "For Cedar, where is the calibration ledger?",
            "Use registered memory tools and cite exact source episode IDs.",
            "Return JSON with answer, citations, and needs_clarification.",
            ChatCompletionsToolProfile::RecallOnly,
            ChatCompletionBudgets {
                context_tokens: 256,
                tool_calls: 2,
                turns: 1,
                output_tokens: 128,
                wall_time: std::time::Duration::from_secs(5),
            },
        )
        .expect("fake model returns one valid answer");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["temperature"], 1.0);
    assert_eq!(requests[0]["top_p"], 0.95);
    assert_eq!(requests[0]["top_k"], 20);
    assert_eq!(requests[0]["min_p"], 0.0);
    assert_eq!(requests[0]["presence_penalty"], 0.0);
    assert_eq!(requests[0]["repeat_penalty"], 1.0);
}

#[test]
#[ignore = "opt-in loopback smoke; configure any local OpenAI-compatible Chat Completions server"]
fn local_chat_completions_smoke_executes_a_public_memory_tool() {
    let endpoint = std::env::var("MEMORY_EVAL_CHAT_COMPLETIONS_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:1234/v1".to_owned());
    let model = std::env::var("MEMORY_EVAL_CHAT_COMPLETIONS_MODEL")
        .expect("set MEMORY_EVAL_CHAT_COMPLETIONS_MODEL to the local server's exact model ID");
    let mut memory = FakeMemorySession::read_only();
    let mut config = ChatCompletionsConfig::new(endpoint, model);
    if let Ok(path) = std::env::var("MEMORY_EVAL_CHAT_COMPLETIONS_RESPONSE_FORMAT") {
        let response_format: Value = serde_json::from_slice(
            &std::fs::read(path).expect("read optional response-format configuration"),
        )
        .expect("optional response-format configuration is JSON");
        config = config.with_response_format(Some(response_format));
    }
    let agent = ChatCompletionsAgent::new(config).expect("configured endpoint is numeric loopback");

    let observation = agent
        .run_phase(
            &mut memory,
            "Run the synthetic connectivity probe. Query memory for 'smoke probe' with budget 1. After the tool reply, answer with JSON: {\"answer\":\"smoke-ok\",\"citations\":[\"episode:cedar\"],\"needs_clarification\":false}.",
            "You are testing a local chat-completions tool-call connection.",
            "Call assemble_context exactly once. Then return only JSON with exactly these fields: answer as a string, citations as an array of strings containing exact source_episode IDs, and needs_clarification as a boolean. Never encode citations as objects and do not add markdown.",
            ChatCompletionsToolProfile::RecallOnly,
            ChatCompletionBudgets {
                context_tokens: 16_384,
                tool_calls: 2,
                turns: 3,
                output_tokens: 512,
                wall_time: std::time::Duration::from_secs(240),
            },
        )
        .expect("local server returns a normalized observation");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::Completed,
        "status={:?}, turns={}, tool_calls={}, wall_time_ms={}, trace={:?}",
        observation.status,
        observation.turns_used,
        observation.tool_calls,
        observation.wall_time_ms,
        observation.trace
    );
    assert_eq!(observation.tool_calls, 1);
    assert_eq!(memory.calls.len(), 1);
    assert_eq!(memory.calls[0].0, "assemble_context");
    assert!(
        observation
            .memory_source_episode_ids
            .contains(&"episode:cedar".to_owned())
    );
}

#[test]
fn chat_completions_runner_records_malformed_provider_output_as_a_model_failure() {
    let server = MockChatServer::with_responses(vec![json!({
        "model": "test-model",
        "choices": [{"message": {"role": "assistant", "content": "{}"}}]
    })]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("model errors are represented as trial observations");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::ModelFailure
    );
    assert_eq!(observation.turns_used, 1);
    assert_eq!(
        observation.trace.last().unwrap().kind,
        TraceEventKind::ModelFailure
    );
    assert_eq!(
        serde_json::to_value(&observation)
            .unwrap()
            .pointer("/trace/1/error_class"),
        Some(&json!("invalid_response"))
    );
    assert!(memory.calls.is_empty());
    let _ = server.requests();
}

#[test]
fn chat_completions_runner_records_only_http_status_for_provider_rejection() {
    let server = MockChatServer::with_http_responses(vec![(
        429,
        json!({
            "error": {"message": "PRIVATE_PROVIDER_ERROR_SENTINEL"}
        }),
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("provider errors are represented as trial observations");

    assert_eq!(observation.terminal_http_status_code, Some(429));
    assert_eq!(observation.terminal_transport_stage, None);
    let serialized = serde_json::to_value(&observation).expect("serialize observation");
    assert_eq!(serialized.pointer("/terminal_http_status_code"), None);
    assert!(
        !serialized
            .to_string()
            .contains("PRIVATE_PROVIDER_ERROR_SENTINEL")
    );
    assert_eq!(
        serialized.pointer("/trace/1/error_class"),
        Some(&json!("transport"))
    );
    assert_eq!(serialized.pointer("/terminal_transport_stage"), None);
    assert!(memory.calls.is_empty());
    let _ = server.requests();
}

#[test]
fn chat_completions_runner_distinguishes_request_send_failure_without_retaining_content() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve local endpoint");
    let address = listener.local_addr().expect("read reserved endpoint");
    drop(listener);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        format!("http://{address}/v1"),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("transport errors are represented as trial observations");

    assert_eq!(observation.terminal_http_status_code, None);
    assert_eq!(
        observation.terminal_transport_stage,
        Some(eval_harness::paired_agent::TerminalTransportStage::RequestSend)
    );
    assert!(memory.calls.is_empty());
}

#[test]
fn chat_completions_runner_distinguishes_response_body_read_failure_without_retaining_content() {
    let private_body = b"PRIVATE_PARTIAL_PROVIDER_BODY_SENTINEL";
    let server = MockChatServer::with_truncated_body(private_body.len() + 32, private_body);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("transport errors are represented as trial observations");

    assert_eq!(observation.terminal_http_status_code, None);
    assert_eq!(
        observation.terminal_transport_stage,
        Some(eval_harness::paired_agent::TerminalTransportStage::ResponseBodyRead)
    );
    let serialized = serde_json::to_string(&observation).expect("serialize observation");
    assert!(!serialized.contains("PRIVATE_PARTIAL_PROVIDER_BODY_SENTINEL"));
    assert!(!serialized.contains("terminal_transport_stage"));
    assert!(memory.calls.is_empty());
    let _ = server.requests();
}

#[test]
fn chat_completions_runner_records_a_trace_event_for_a_clarification_answer() {
    let server = MockChatServer::with_responses(vec![model_response(
        json!({
            "role": "assistant",
            "content": "{\"answer\":\"unknown\",\"citations\":[],\"needs_clarification\":true}"
        }),
        8,
        3,
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("valid clarification answers are represented as trial observations");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    assert_eq!(observation.user_clarifications, 1);
    assert_eq!(
        observation
            .trace
            .iter()
            .filter(|event| event.kind == TraceEventKind::Clarification)
            .count(),
        1
    );
    let _ = server.requests();
}

#[test]
fn chat_completions_runner_classifies_non_json_final_answer_without_retaining_it() {
    let server = MockChatServer::with_responses(vec![model_response(
        json!({
            "role": "assistant",
            "content": "PRIVATE_MODEL_RESPONSE_SENTINEL"
        }),
        12,
        4,
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("model errors are represented as trial observations");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::ModelFailure
    );
    assert_eq!(
        serde_json::to_value(&observation)
            .unwrap()
            .pointer("/trace/2/error_class"),
        Some(&json!("final_answer_not_json"))
    );
    assert!(
        !serde_json::to_string(&observation)
            .unwrap()
            .contains("PRIVATE_MODEL_RESPONSE_SENTINEL")
    );
    assert!(memory.calls.is_empty());
    let _ = server.requests();
}

#[test]
fn chat_completions_runner_classifies_final_answer_schema_mismatch_without_retaining_it() {
    let server = MockChatServer::with_responses(vec![model_response(
        json!({
            "role": "assistant",
            "content": "{\"answer\":\"PRIVATE_MODEL_RESPONSE_SENTINEL\",\"citations\":[{\"source_episode\":\"episode:cedar\"}],\"needs_clarification\":false}"
        }),
        12,
        18,
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("model errors are represented as trial observations");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::ModelFailure
    );
    assert_eq!(
        serde_json::to_value(&observation)
            .unwrap()
            .pointer("/trace/2/error_class"),
        Some(&json!("final_answer_schema_mismatch"))
    );
    assert!(
        !serde_json::to_string(&observation)
            .unwrap()
            .contains("PRIVATE_MODEL_RESPONSE_SENTINEL")
    );
    assert!(memory.calls.is_empty());
    let _ = server.requests();
}

#[test]
fn chat_completions_runner_classifies_empty_formation_completion() {
    let server = MockChatServer::with_responses(vec![model_response(
        json!({"role": "assistant", "content": ""}),
        12,
        0,
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession {
        calls: Vec::new(),
        reply_override: None,
        schemas: json!([
            {"name":"ingest", "inputSchema":{"type":"object"}},
            {"name":"extract", "inputSchema":{"type":"object"}}
        ]),
    };

    let observation = agent
        .run_phase(
            &mut memory,
            "formation task",
            "system",
            "rules",
            ChatCompletionsToolProfile::SourceAuthorityFormation,
            budgets(),
        )
        .expect("model errors are represented as trial observations");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::ModelFailure
    );
    assert_eq!(
        serde_json::to_value(&observation)
            .unwrap()
            .pointer("/trace/2/error_class"),
        Some(&json!("empty_formation_completion"))
    );
    assert!(memory.calls.is_empty());
    let _ = server.requests();
}

#[test]
fn chat_completions_runner_never_executes_a_tool_outside_the_phase_allow_list() {
    let server = MockChatServer::with_responses(vec![model_response(
        json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "call-forbidden",
                "type": "function",
                "function": {"name": "invalidate", "arguments": "{}"}
            }]
        }),
        10,
        3,
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("forbidden calls are model failures, not runner crashes");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::ModelFailure
    );
    assert_eq!(
        observation.trace.last().unwrap().error_class,
        Some(eval_harness::paired::NormalizedErrorClass::Unauthorized)
    );
    assert!(memory.calls.is_empty());
    let _ = server.requests();
}

#[test]
fn source_authority_formation_accepts_a_non_json_completion_message() {
    let schemas = json!([
        {"name":"ingest", "inputSchema":{"type":"object"}},
        {"name":"extract", "inputSchema":{"type":"object"}}
    ]);
    let server = MockChatServer::with_responses(vec![model_response(
        json!({"role":"assistant", "content":"Saved the requested source."}),
        9,
        5,
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession {
        calls: Vec::new(),
        reply_override: None,
        schemas,
    };

    let observation = agent
        .run_phase(
            &mut memory,
            "remember this source",
            "system",
            "rules",
            ChatCompletionsToolProfile::SourceAuthorityFormation,
            budgets(),
        )
        .expect("formation completion can be a concise status message");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    assert_eq!(
        observation.final_answer.as_deref(),
        Some("Saved the requested source.")
    );
    let _ = server.requests();
}

#[test]
fn source_authority_control_candidate_keeps_the_write_profile() {
    use eval_harness::paired::RunArm;

    let candidate_without_operator_import =
        ChatCompletionsToolProfile::source_authority_formation_for_arm(RunArm::Candidate, false);
    let candidate_with_operator_import =
        ChatCompletionsToolProfile::source_authority_formation_for_arm(RunArm::Candidate, true);
    let incumbent =
        ChatCompletionsToolProfile::source_authority_formation_for_arm(RunArm::Incumbent, false);

    assert_eq!(
        candidate_without_operator_import.enabled_tools(),
        ["ingest", "extract"]
    );
    assert_eq!(
        candidate_with_operator_import.enabled_tools(),
        ["assemble_context"]
    );
    assert_eq!(incumbent.enabled_tools(), ["ingest", "extract"]);
}

#[test]
fn extract_evidence_telemetry_uses_the_product_default_when_the_argument_is_omitted() {
    let schemas = json!([
        {"name":"ingest", "inputSchema":{"type":"object"}},
        {"name":"extract", "inputSchema":{"type":"object"}}
    ]);
    let server = MockChatServer::with_responses(vec![
        model_response(
            json!({
                "role":"assistant",
                "tool_calls":[{
                    "id":"call-extract",
                    "type":"function",
                    "function":{"name":"extract", "arguments":"{\"episode_id\":\"episode:test\"}"}
                }]
            }),
            12,
            5,
        ),
        model_response(json!({"role":"assistant", "content":"Saved."}), 18, 3),
    ]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession {
        calls: Vec::new(),
        reply_override: None,
        schemas,
    };

    let observation = agent
        .run_phase(
            &mut memory,
            "extract the episode",
            "system",
            "rules",
            ChatCompletionsToolProfile::SourceAuthorityFormation,
            budgets(),
        )
        .expect("registered extraction call completes");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    assert_eq!(observation.extract_fact_evidence.len(), 1);
    assert!(observation.extract_fact_evidence[0].requested);
    let requests = server.requests();
    assert_eq!(requests[1]["max_tokens"], 123);
}

#[test]
fn turn_budget_exhaustion_is_recorded_in_the_normalized_trace() {
    let server = MockChatServer::with_responses(vec![model_response(
        json!({
            "role":"assistant",
            "tool_calls":[{
                "id":"call-read",
                "type":"function",
                "function":{"name":"assemble_context", "arguments":"{}"}
            }]
        }),
        12,
        5,
    )]);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();
    let mut budgets = budgets();
    budgets.turns = 1;

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets,
        )
        .expect("turn budget is a measured trial outcome");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::BudgetExceeded
    );
    assert_eq!(
        observation.trace.last().unwrap().kind,
        TraceEventKind::BudgetLimitReached
    );
    let _ = server.requests();
}

fn budgets() -> ChatCompletionBudgets {
    ChatCompletionBudgets {
        context_tokens: 256,
        tool_calls: 2,
        turns: 3,
        output_tokens: 128,
        wall_time: std::time::Duration::from_secs(5),
    }
}

#[test]
fn chat_completions_runner_configuration_cannot_send_task_data_to_non_loopback_hosts() {
    let result = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        "http://example.com/v1".to_owned(),
        "test-model",
    ));

    assert!(result.is_err());
}

#[test]
fn chat_completions_client_refuses_redirects_even_to_another_loopback_url() {
    let (endpoint, server) = redirecting_server();
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(endpoint, "test-model"))
        .expect("loopback endpoint is valid");
    let mut memory = FakeMemorySession::read_only();

    let observation = agent
        .run_phase(
            &mut memory,
            "question",
            "system",
            "rules",
            ChatCompletionsToolProfile::RecallOnly,
            budgets(),
        )
        .expect("redirect refusal is captured as a model failure");

    assert_eq!(
        observation.status,
        eval_harness::paired::TrialStatus::ModelFailure
    );
    assert_eq!(server.join().expect("redirect server exits"), 0);
    assert!(memory.calls.is_empty());
}
