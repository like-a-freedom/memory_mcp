# File Log Sink (`MEMORY_LOG_FILE`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional file-based log output controlled by `MEMORY_LOG_FILE` env var, so MCP hosts that don't expose stderr can still capture diagnostic logs.

**Architecture:** A process-global `OnceLock<Mutex<File>>` sink in `logging.rs`. All `StdoutLogger` instances check the global sink in `log()`; if installed, lines go to the file instead of stderr. The env var is read once in `runner.rs` (composition root) before any logging occurs. On open failure, a warn is emitted to stderr and the process continues with stderr logging.

**Tech Stack:** Rust std (`std::fs::File`, `std::sync::OnceLock`, `std::sync::Mutex`), `tempfile` (dev-dep, already present).

**Spec:** Design decisions from grilling session (this conversation). No separate spec file.

## Global Constraints

- No new dependencies (use std only; `tempfile` already in dev-deps).
- No `unwrap()` in production code — use `unwrap_or_else` for mutex recovery.
- `StdoutLogger::new()` signature unchanged — no breaking changes to existing callers.
- Progress sinks (`JsonLineProgressSink`, `CliProgressSink`, `IndicatifProgressReporter`) are NOT affected.
- No log rotation — append only.
- Empty/whitespace-only `MEMORY_LOG_FILE` is treated as unset.
- Parent directories are NOT created — only the file itself (`create + append`).
- Flush after every line (crash safety).
- `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` must pass.
- `cargo fmt --all --check` must pass.

---

### Task 1: Process-global file sink in `logging.rs`

**Files:**
- Modify: `crates/memory-mcp/src/logging.rs`

**Interfaces:**
- Consumes: nothing from other tasks
- Produces: `pub fn install_log_file(path: &str) -> Result<(), std::io::Error>` — called by Task 2

- [ ] **Step 1: Write the failing test**

Create a new integration test file `crates/memory-mcp/tests/file_log_sink.rs`.
Integration tests run as separate binaries, so the process-global `OnceLock`
won't interfere with unit tests in the main crate.

```rust
//! Integration test for the process-global file log sink.
//!
//! This test installs the global sink and verifies that `StdoutLogger`
//! writes to the file instead of stderr. It runs in its own binary to
//! avoid polluting other tests with the process-global `OnceLock`.

use std::collections::HashMap;

use memory_mcp::logging::{LogLevel, StdoutLogger, install_log_file};
use serde_json::json;

#[test]
fn install_log_file_writes_events_to_file() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let log_path = dir.path().join("test.log");
    let path_str = log_path.to_str().expect("valid utf8 path");

    // Install the global file sink
    install_log_file(path_str).expect("install_log_file should succeed");

    // Second install must fail (OnceLock already set)
    let result = install_log_file(path_str);
    assert!(result.is_err(), "second install should fail");

    // Create a logger and emit an event
    let logger = StdoutLogger::new("info");
    let mut event = HashMap::new();
    event.insert("op".to_string(), json!("test.file_sink"));
    event.insert("value".to_string(), json!(42u64));
    logger.log(event, LogLevel::Info);

    // Read the file and verify content
    let content = std::fs::read_to_string(&log_path).expect("read log file");
    assert!(content.contains("op=test.file_sink"), "missing op field: {content}");
    assert!(content.contains("value=42"), "missing value field: {content}");
    assert!(content.contains("INFO"), "missing level: {content}");
}

#[test]
fn install_log_file_fails_for_nonexistent_directory() {
    // open() fails before OnceLock::set() is reached, so this test is
    // independent of whether another test already installed the sink.
    let result = install_log_file("/nonexistent_dir_xyz_12345/test.log");
    assert!(result.is_err(), "should fail for missing parent directory");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p memory_mcp --test file_log_sink -- --nocapture`
Expected: FAIL — `install_log_file` not found (compile error).

- [ ] **Step 3: Write minimal implementation**

Add these imports at the top of `crates/memory-mcp/src/logging.rs` (after existing `use` statements):

```rust
use std::fs::{File, OpenOptions};
use std::sync::OnceLock;
```

Add the global sink and installer function after the `WarnTracker` struct (before `StdoutLogger`):

```rust
/// Process-global file sink. When installed, all `StdoutLogger` instances
/// write to this file instead of stderr. Set once at startup via
/// [`install_log_file`]; never unset for the process lifetime.
static LOG_FILE_SINK: OnceLock<Mutex<File>> = OnceLock::new();

/// Installs a file-based log sink for the entire process.
///
/// Opens the file in append mode, creating it if it does not exist.
/// Parent directories are NOT created. Returns `Err` if the file cannot
/// be opened (missing directory, permission denied, etc.).
///
/// This function is idempotent-safe: calling it a second time returns
/// `Err` with `ErrorKind::AlreadyExists` (the first installation wins).
pub fn install_log_file(path: &str) -> Result<(), io::Error> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    LOG_FILE_SINK
        .set(Mutex::new(file))
        .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "log file sink already installed"))
}
```

Modify `StdoutLogger::log()` to check the global sink:

```rust
pub fn log(&self, event: HashMap<String, Value>, level: LogLevel) {
    if level < self.level {
        return;
    }

    let line = Self::format_event_line(&event, level);

    // Write to file sink if installed; otherwise fall through to stderr.
    if let Some(sink) = LOG_FILE_SINK.get() {
        let mut file = sink.lock().unwrap_or_else(|poison| poison.into_inner());
        let _ = file.write_all(line.as_bytes());
        let _ = file.write_all(b"\n");
        let _ = file.flush();
        return;
    }

    let mut stderr = io::stderr();
    let _ = stderr.write_all(line.as_bytes());
    let _ = stderr.write_all(b"\n");
    let _ = stderr.flush();
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p memory_mcp --test file_log_sink -- --nocapture`
Expected: PASS (all three tests).

- [ ] **Step 5: Run full test suite to check no regressions**

Run: `cargo test -p memory_mcp`
Expected: All existing tests pass. The integration test runs in its own binary,
so the process-global sink does not affect unit tests.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/logging.rs crates/memory-mcp/tests/file_log_sink.rs
git commit -m "feat(logging): add process-global file sink via install_log_file"
```

---

### Task 2: Wire `MEMORY_LOG_FILE` env var in `runner.rs`

**Files:**
- Modify: `crates/memory-mcp/src/runner.rs`

**Interfaces:**
- Consumes: `crate::logging::install_log_file(path: &str) -> Result<(), std::io::Error>` from Task 1
- Produces: nothing (terminal wiring)

- [ ] **Step 1: Write the failing test**

Add at the end of `mod tests` in `crates/memory-mcp/src/runner.rs`:

```rust
#[test]
fn resolve_log_file_path_trims_and_rejects_empty() {
    assert_eq!(
        super::resolve_log_file_path("  /tmp/test.log  "),
        Some("/tmp/test.log".to_string())
    );
    assert_eq!(super::resolve_log_file_path(""), None);
    assert_eq!(super::resolve_log_file_path("   "), None);
    assert_eq!(
        super::resolve_log_file_path("/var/log/memory.log"),
        Some("/var/log/memory.log".to_string())
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p memory_mcp resolve_log_file_path -- --nocapture`
Expected: FAIL — `resolve_log_file_path` not found (compile error).

- [ ] **Step 3: Write minimal implementation**

Add imports at the top of `crates/memory-mcp/src/runner.rs`. Replace the existing
`use crate::logging::StdoutLogger;` line with:

```rust
use std::collections::HashMap;

use serde_json::json;

use crate::logging::{LogLevel, StdoutLogger, install_log_file};
```

Add the helper function before `pub async fn run()`:

```rust
/// Reads `MEMORY_LOG_FILE` from the environment. Returns `Some(trimmed_path)`
/// if the variable is set and non-empty after trimming; `None` otherwise.
fn resolve_log_file_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
```

Modify `pub async fn run()` to install the file sink before `log_startup`:

```rust
pub async fn run() -> Result<(), ExitCode> {
    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
    let logger = StdoutLogger::new(&log_level);

    // Install file log sink if configured. Must happen before log_startup
    // so the very first event goes to the file.
    if let Ok(raw_path) = std::env::var("MEMORY_LOG_FILE") {
        if let Some(path) = resolve_log_file_path(&raw_path) {
            if let Err(err) = install_log_file(&path) {
                // Fallback: warn to stderr (sink not installed, so stderr works).
                let mut event = HashMap::new();
                event.insert("op".to_string(), json!("main.log_file_open_failed"));
                event.insert("path".to_string(), json!(&path));
                event.insert("error".to_string(), json!(err.to_string()));
                logger.log(event, LogLevel::Warn);
            }
        }
    }

    let cli = Cli::parse();

    let startup_ts = chrono::Utc::now();
    log_startup(&logger, mode_label(&cli));

    let outcome = dispatch(&logger, cli).await;

    let duration = chrono::Utc::now().signed_duration_since(startup_ts);
    log_session_duration(&logger, duration.num_seconds());

    outcome
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p memory_mcp resolve_log_file_path -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p memory_mcp`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/runner.rs
git commit -m "feat(runner): read MEMORY_LOG_FILE and install file log sink at startup"
```

---

### Task 3: Update README documentation

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: nothing
- Produces: nothing (documentation only)

- [ ] **Step 1: Add `MEMORY_LOG_FILE` to the Advanced runtime overrides table**

In `README.md`, in the "Advanced runtime overrides" table (around line 556–575), add a new row after the `RUST_LOG` row:

```markdown
| `MEMORY_LOG_FILE` | path | unset | Write structured log events to this file instead of stderr; the file is created if missing (parent directory must exist), opened in append mode, and flushed after every line; on open failure the process falls back to stderr with a warning |
```

- [ ] **Step 2: Add a note in the "Logging levels and what they cover" section**

After the existing paragraph about `.env` (around line 863), add:

```markdown
When `MEMORY_LOG_FILE` is set to a non-empty path, all structured log events are written to that file instead of stderr. This is useful for MCP hosts that do not expose the server's stderr. The file is opened in append mode (no rotation); the parent directory must already exist. If the file cannot be opened, a warning is emitted to stderr and logging continues there.
```

- [ ] **Step 3: Update the CLI Mode "Output Format" section**

In the "Output Format" section (around line 1164), change:

```markdown
Structured log events go to **stderr** (controlled by `RUST_LOG`).
```

to:

```markdown
Structured log events go to **stderr** (controlled by `RUST_LOG`), or to the
file named by `MEMORY_LOG_FILE` when that variable is set.
```

- [ ] **Step 4: Verify formatting**

Run: `cargo fmt --all --check`
Expected: PASS (markdown is not affected by rustfmt, but verify no accidental .rs changes).

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document MEMORY_LOG_FILE env var for file-based logging"
```

---

### Task 4: Final validation

**Files:** none (verification only)

- [ ] **Step 1: Run clippy**

Run: `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2: Run fmt check**

Run: `cargo fmt --all --check`
Expected: zero diff.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p memory_mcp`
Expected: all tests pass.

- [ ] **Step 4: Manual smoke test**

```bash
MEMORY_LOG_FILE=/tmp/memory_mcp_test.log cargo run --quiet --bin memory_mcp -- init --target vscode > /dev/null 2>&1
cat /tmp/memory_mcp_test.log
```

Expected: file contains structured log lines with `op=main.startup` and `op=main.session_duration`.

```bash
# Verify stderr is empty when file sink is active
MEMORY_LOG_FILE=/tmp/memory_mcp_test2.log cargo run --quiet --bin memory_mcp -- init --target vscode 2>/tmp/stderr_capture.txt > /dev/null
cat /tmp/stderr_capture.txt
```

Expected: stderr capture is empty (all logs went to file).

```bash
# Verify fallback on bad path
MEMORY_LOG_FILE=/nonexistent_dir/test.log cargo run --quiet --bin memory_mcp -- init --target vscode > /dev/null 2>&1
```

Expected: process exits 0, warning about `log_file_open_failed` on stderr.
