use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::EvalError;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STDOUT_LINE_BYTES: usize = 1_048_576;
const MAX_STDERR_SAMPLE_BYTES: usize = 32_768;

enum ReaderMessage {
    Line(String),
    Error(String),
}

pub struct StdioSession {
    pub(crate) launch_profile: BTreeMap<String, String>,
    child: Arc<Mutex<Child>>,
    deadline_stop: Option<mpsc::Sender<()>>,
    deadline_worker: Option<JoinHandle<()>>,
    deadline: Option<Instant>,
    stdin: Option<ChildStdin>,
    responses: Option<mpsc::Receiver<ReaderMessage>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
    stderr_sample: Arc<Mutex<Vec<u8>>>,
    pending: BTreeMap<i64, Value>,
    next_id: i64,
    startup_timeout: Duration,
    call_timeout: Duration,
    shutdown_timeout: Duration,
    initialized: Value,
    tool_schemas: Value,
    child_reaped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredToolResponse {
    pub response: Value,
    pub elapsed_us: u64,
    pub compact_jsonrpc_bytes: usize,
}

impl StdioSession {
    pub fn spawn(binary: &Path, store: &Path) -> Result<Self, EvalError> {
        Self::spawn_with_timeouts(
            binary,
            store,
            STARTUP_TIMEOUT,
            CALL_TIMEOUT,
            SHUTDOWN_TIMEOUT,
        )
    }

    /// All requests and owned process lifetime share this absolute run deadline.
    pub fn spawn_with_deadline(
        binary: &Path,
        store: &Path,
        claim_stage: &str,
        deadline: Instant,
    ) -> Result<Self, EvalError> {
        let stage = match claim_stage {
            "default" => None,
            "disabled" | "shadow" | "relations" | "evidence" => Some(claim_stage),
            _ => {
                return Err(EvalError::InvalidConfig(
                    "unsupported claim rollout stage".into(),
                ));
            }
        };
        Self::spawn_inner(
            binary,
            store,
            stage,
            None,
            (STARTUP_TIMEOUT, CALL_TIMEOUT, SHUTDOWN_TIMEOUT),
            Some(deadline),
        )
    }

    pub fn spawn_with_claim_stage(
        binary: &Path,
        store: &Path,
        claim_stage: &str,
    ) -> Result<Self, EvalError> {
        if !matches!(
            claim_stage,
            "disabled" | "shadow" | "relations" | "evidence"
        ) {
            return Err(EvalError::InvalidConfig(format!(
                "unsupported claim rollout stage `{claim_stage}`"
            )));
        }
        Self::spawn_inner(
            binary,
            store,
            Some(claim_stage),
            None,
            (STARTUP_TIMEOUT, CALL_TIMEOUT, SHUTDOWN_TIMEOUT),
            None,
        )
    }

    pub fn spawn_with_claim_stage_and_cwd(
        binary: &Path,
        store: &Path,
        claim_stage: &str,
        working_directory: &Path,
    ) -> Result<Self, EvalError> {
        if !matches!(
            claim_stage,
            "disabled" | "shadow" | "relations" | "evidence"
        ) {
            return Err(EvalError::InvalidConfig(format!(
                "unsupported claim rollout stage `{claim_stage}`"
            )));
        }
        if !working_directory.is_absolute() || !working_directory.is_dir() {
            return Err(EvalError::InvalidConfig(
                "stdio memory working directory must be an existing absolute directory".into(),
            ));
        }
        Self::spawn_inner(
            binary,
            store,
            Some(claim_stage),
            Some(working_directory),
            (STARTUP_TIMEOUT, CALL_TIMEOUT, SHUTDOWN_TIMEOUT),
            None,
        )
    }

    pub fn spawn_with_timeouts(
        binary: &Path,
        store: &Path,
        startup_timeout: Duration,
        call_timeout: Duration,
        shutdown_timeout: Duration,
    ) -> Result<Self, EvalError> {
        Self::spawn_inner(
            binary,
            store,
            None,
            None,
            (startup_timeout, call_timeout, shutdown_timeout),
            None,
        )
    }

    fn spawn_inner(
        binary: &Path,
        store: &Path,
        claim_stage: Option<&str>,
        working_directory: Option<&Path>,
        timeouts: (Duration, Duration, Duration),
        deadline: Option<Instant>,
    ) -> Result<Self, EvalError> {
        let (startup_timeout, call_timeout, shutdown_timeout) = timeouts;
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Err(EvalError::Suite(
                "evaluation deadline expired before server startup".into(),
            ));
        }
        std::fs::create_dir_all(store).map_err(|source| EvalError::Io {
            path: store.to_path_buf(),
            source,
        })?;
        let mut command = Command::new(binary);
        command.arg("serve");
        if let Some(working_directory) = working_directory {
            command.current_dir(working_directory);
        }
        configure_embedded_memory_command(&mut command, store, claim_stage);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let launch_profile = launch_profile(&command);
        let mut child = command.spawn().map_err(|source| EvalError::Io {
            path: binary.to_path_buf(),
            source,
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EvalError::Suite("stdio server did not expose a piped stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EvalError::Suite("stdio server did not expose a piped stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| EvalError::Suite("stdio server did not expose a piped stderr".into()))?;

        let (sender, responses) = mpsc::sync_channel(64);
        let stdout_reader = thread::spawn(move || read_stdout(stdout, sender));
        let stderr_sample = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = thread::spawn({
            let sample = Arc::clone(&stderr_sample);
            move || drain_stderr(stderr, sample)
        });
        let child = Arc::new(Mutex::new(child));
        let (deadline_stop, deadline_worker) = if let Some(limit) = deadline {
            let (stop, stopped) = mpsc::channel();
            let owned_child = Arc::clone(&child);
            let worker = thread::spawn(move || {
                if matches!(
                    stopped.recv_timeout(limit.saturating_duration_since(Instant::now())),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    let mut child = owned_child
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let _ = child.kill();
                    let _ = child.wait();
                }
            });
            (Some(stop), Some(worker))
        } else {
            (None, None)
        };
        let mut session = Self {
            launch_profile,
            child,
            deadline_stop,
            deadline_worker,
            deadline,
            stdin: Some(stdin),
            responses: Some(responses),
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            stderr_sample,
            pending: BTreeMap::new(),
            next_id: 1,
            startup_timeout,
            call_timeout,
            shutdown_timeout,
            initialized: Value::Null,
            tool_schemas: Value::Null,
            child_reaped: false,
        };
        session.initialize_and_validate()?;
        Ok(session)
    }

    pub fn initialized(&self) -> &Value {
        &self.initialized
    }

    pub fn tool_schemas(&self) -> &Value {
        &self.tool_schemas
    }

    pub fn stderr_excerpt(&self) -> String {
        let sample = self
            .stderr_sample
            .lock()
            .map(|bytes| bytes.clone())
            .unwrap_or_default();
        String::from_utf8_lossy(&sample).into_owned()
    }

    pub fn call_method(&mut self, method: &str, params: Value) -> Result<Value, EvalError> {
        self.request(method, params, self.call_timeout)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), EvalError> {
        let message = json!({"jsonrpc":"2.0","method":method,"params":params});
        self.write_message(&message)
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, EvalError> {
        let response =
            self.call_method("tools/call", json!({"name":name,"arguments":arguments}))?;
        if response.get("error").is_some() {
            return Err(EvalError::Suite(format!(
                "MCP tools/call returned JSON-RPC error: {}",
                response["error"]
            )));
        }
        if response.pointer("/result/isError").and_then(Value::as_bool) == Some(true) {
            return Err(EvalError::Suite(format!(
                "MCP tool `{name}` returned isError=true"
            )));
        }
        Ok(response)
    }

    pub fn call_tool_measured(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<MeasuredToolResponse, EvalError> {
        let started = Instant::now();
        let response = self.call_tool(name, arguments)?;
        let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let compact_jsonrpc_bytes = serde_json::to_vec(&response)?.len();
        Ok(MeasuredToolResponse {
            compact_jsonrpc_bytes,
            response,
            elapsed_us,
        })
    }

    pub fn shutdown(&mut self) -> Result<(), EvalError> {
        self.stdin.take();
        let deadline = self
            .deadline
            .map_or(Instant::now() + self.shutdown_timeout, |limit| {
                limit.min(Instant::now() + self.shutdown_timeout)
            });
        loop {
            let status = self
                .child
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .try_wait();
            match status {
                Ok(Some(status)) => {
                    self.child_reaped = true;
                    self.join_readers();
                    if status.success() {
                        return Ok(());
                    }
                    return Err(EvalError::Suite(format!(
                        "stdio server exited with status {status}"
                    )));
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                Ok(None) => {
                    self.kill_and_reap();
                    self.join_readers();
                    return Err(EvalError::Suite("stdio server shutdown timed out".into()));
                }
                Err(error) => {
                    self.kill_and_reap();
                    self.join_readers();
                    return Err(EvalError::Suite(format!(
                        "failed to wait for stdio server: {error}"
                    )));
                }
            }
        }
    }

    fn initialize_and_validate(&mut self) -> Result<(), EvalError> {
        let initialized = self.request(
            "initialize",
            json!({
                "protocolVersion":"2025-11-25",
                "capabilities":{},
                "clientInfo":{"name":"memory-decisions-eval","version":"1"}
            }),
            self.startup_timeout,
        )?;
        if initialized.get("error").is_some() {
            return Err(EvalError::Suite(format!(
                "MCP initialize failed: {}",
                initialized["error"]
            )));
        }
        self.initialized = initialized
            .pointer("/result")
            .cloned()
            .ok_or_else(|| EvalError::InvalidInput("initialize response lacks result".into()))?;
        self.notify("notifications/initialized", json!({}))?;

        let listed = self.request("tools/list", json!({}), self.startup_timeout)?;
        let tools = listed
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .ok_or_else(|| EvalError::InvalidInput("tools/list lacks result.tools array".into()))?;
        for required in ["ingest", "extract", "assemble_context"] {
            let tool = tools
                .iter()
                .find(|tool| tool.get("name").and_then(Value::as_str) == Some(required))
                .ok_or_else(|| {
                    EvalError::InvalidInput(format!("server does not advertise `{required}`"))
                })?;
            if tool.get("inputSchema").is_none() {
                return Err(EvalError::InvalidInput(format!(
                    "server does not advertise the `{required}` input schema"
                )));
            }
        }
        self.tool_schemas = Value::Array(tools.clone());
        Ok(())
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, EvalError> {
        let started = Instant::now();
        let effective = self.deadline.map_or(timeout, |limit| {
            timeout.min(limit.saturating_duration_since(started))
        });
        let result = self.request_inner(method, params, effective);
        if result.is_err()
            && (started.elapsed() >= effective
                || self.deadline.is_some_and(|limit| Instant::now() >= limit))
        {
            return Err(EvalError::Timeout {
                operation: format!("MCP method `{method}`"),
                timeout_ms: u64::try_from(effective.as_millis()).unwrap_or(u64::MAX),
            });
        }
        result
    }

    fn request_inner(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, EvalError> {
        if self.deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Err(EvalError::Suite(
                "whole-run deadline exhausted before MCP request".into(),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.write_message(&message)?;
        if let Some(response) = self.pending.remove(&id) {
            return Ok(response);
        }
        let deadline = self.deadline.map_or(Instant::now() + timeout, |limit| {
            limit.min(Instant::now() + timeout)
        });
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EvalError::Suite(format!(
                    "timed out after {} ms waiting for MCP method `{method}`",
                    timeout.as_millis()
                )));
            }
            let response_receiver = self
                .responses
                .as_ref()
                .ok_or_else(|| EvalError::Suite("stdio server response reader is closed".into()))?;
            let message = match response_receiver.recv_timeout(remaining) {
                Ok(ReaderMessage::Line(line)) => line,
                Ok(ReaderMessage::Error(error)) => return Err(EvalError::Suite(error)),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(EvalError::Suite(format!(
                        "timed out after {} ms waiting for MCP method `{method}`",
                        timeout.as_millis()
                    )));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(EvalError::Suite(
                        "stdio server response channel disconnected".into(),
                    ));
                }
            };
            let response: Value = serde_json::from_str(&message)?;
            let Some(response_id) = response.get("id").and_then(Value::as_i64) else {
                // Server notifications do not complete a request.
                continue;
            };
            if response_id == id {
                return Ok(response);
            }
            if self.pending.len() >= 64 {
                return Err(EvalError::InvalidInput(
                    "stdio server sent too many out-of-order responses".into(),
                ));
            }
            self.pending.insert(response_id, response);
        }
    }

    fn write_message(&mut self, message: &Value) -> Result<(), EvalError> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| EvalError::Suite("stdio server stdin is closed".into()))?;
        serde_json::to_writer(&mut *stdin, message)?;
        stdin
            .write_all(b"\n")
            .and_then(|()| stdin.flush())
            .map_err(|error| EvalError::Suite(format!("failed to write MCP request: {error}")))
    }

    fn kill_and_reap(&mut self) {
        if self.child_reaped {
            return;
        }
        let mut child = self.child.lock().unwrap_or_else(|error| error.into_inner());
        let _ = child.kill();
        let _ = child.wait();
        self.child_reaped = true;
    }

    fn join_readers(&mut self) {
        self.responses.take();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

pub(crate) fn configure_embedded_memory_command(
    command: &mut Command,
    store: &Path,
    claim_stage: Option<&str>,
) {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("TMPDIR", std::env::temp_dir())
        .env("RUST_LOG", "warn")
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", store)
        .env("SURREALDB_DB_NAME", "memory_decisions")
        .env("SURREALDB_NAMESPACE", "main")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("EMBEDDINGS_ENABLED", "false")
        .env("NER_EXTRACTOR", "regex");
    if let Some(claim_stage) = claim_stage {
        command.env("MEMORY_CLAIM_ROLLOUT_STAGE", claim_stage);
    }
}

impl Drop for StdioSession {
    fn drop(&mut self) {
        self.stdin.take();
        self.kill_and_reap();
        self.join_readers();
        if let Some(stop) = self.deadline_stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.deadline_worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_stdout<R: Read>(stdout: R, sender: mpsc::SyncSender<ReaderMessage>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    let mut overlong = false;
    loop {
        let buffer = match reader.fill_buf() {
            Ok([]) => {
                if !line.is_empty() || overlong {
                    let _ = sender.send(ReaderMessage::Error(
                        "stdio server closed stdout on an incomplete JSON-RPC line".into(),
                    ));
                } else {
                    let _ = sender.send(ReaderMessage::Error("stdio server closed stdout".into()));
                }
                return;
            }
            Ok(buffer) => buffer,
            Err(error) => {
                let _ = sender.send(ReaderMessage::Error(format!(
                    "failed to read stdio server stdout: {error}"
                )));
                return;
            }
        };
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |index| index + 1);
        if !overlong {
            if line.len() + consumed > MAX_STDOUT_LINE_BYTES {
                overlong = true;
                line.clear();
            } else {
                line.extend_from_slice(&buffer[..consumed]);
            }
        }
        let ended = newline.is_some();
        reader.consume(consumed);
        if ended {
            if overlong {
                let _ = sender.send(ReaderMessage::Error(format!(
                    "stdio server JSON-RPC line exceeds {MAX_STDOUT_LINE_BYTES} bytes"
                )));
                return;
            }
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let bytes = std::mem::take(&mut line);
            match String::from_utf8(bytes) {
                Ok(line) => {
                    if sender.send(ReaderMessage::Line(line)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(ReaderMessage::Error(format!(
                        "stdio server emitted non-UTF-8 stdout: {error}"
                    )));
                    return;
                }
            }
            overlong = false;
        }
    }
}

fn drain_stderr<R: Read>(mut stderr: R, sample: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0_u8; 8192];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                let Ok(mut sample) = sample.lock() else {
                    return;
                };
                let remaining = MAX_STDERR_SAMPLE_BYTES.saturating_sub(sample.len());
                if remaining > 0 {
                    sample.extend_from_slice(&chunk[..count.min(remaining)]);
                }
            }
        }
    }
}

/// Only explicit, non-secret settings are eligible for an evaluation receipt.
pub(crate) fn launch_profile(command: &std::process::Command) -> BTreeMap<String, String> {
    const PUBLIC_SETTINGS: &[&str] = &[
        "NER_EXTRACTOR",
        "EMBEDDINGS_ENABLED",
        "MEMORY_CLAIM_ROLLOUT_STAGE",
        "SURREALDB_EMBEDDED",
        "SURREALDB_DB_NAME",
        "SURREALDB_NAMESPACE",
        "SURREALDB_TENANT_DB",
        "SURREALDB_TENANT_NAMESPACE",
        "SURREALDB_CONTROL_DB",
        "SURREALDB_CONTROL_NAMESPACE",
    ];
    command
        .get_envs()
        .filter_map(|(key, value)| {
            let key = key.to_str()?;
            if !PUBLIC_SETTINGS.contains(&key) {
                return None;
            }
            Some((key.to_string(), value?.to_str()?.to_string()))
        })
        .collect()
}
