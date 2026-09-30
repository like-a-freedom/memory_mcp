//! Structured logging utilities.
//!
//! This module provides a simple logger with structured event formatting
//! and configurable log levels. Events go to **stderr** (or to the file
//! sink installed at startup), never to stdout: under the stdio transport
//! stdout is reserved for MCP protocol framing.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
use serde_json::Value;

/// Log level for filtering log output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Parses a log level from a string.
    ///
    /// Case-insensitive. Defaults to `Info` for unknown values.
    #[must_use]
    pub fn parse(level: &str) -> Self {
        match level.trim().to_lowercase().as_str() {
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "warn" | "warning" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }

    /// Returns the string representation of the log level.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// The operation name of the HTTP access log.
///
/// Named as a constant because it is the filter key: `RUST_LOG=http=error`
/// selects it, and a test asserting on the literal would otherwise be
/// asserting on a string no code emits.
pub const OP_HTTP_REQUEST: &str = "http.request";

/// Every operation name the HTTP runtime emits.
///
/// The inventory of what `RUST_LOG=http=…` is able to reach. An operation
/// name outside the `http` namespace reads as though the directive applies to
/// it and is not affected by it — the same dishonesty as a function named
/// `tracing_warn` that prints. Keeping the list next to the rule it encodes
/// means the check and the names cannot drift apart silently.
pub const HTTP_OPERATIONS: &[&str] = &[
    OP_HTTP_REQUEST,
    "http.scheduler.failed",
    "http.job.failed",
    "http.job.panicked",
    "http.job.not_run",
    "http.lease.claim_conflict",
    "http.lease.claim_failed",
    "http.lease.claim_terminal",
    "http.lease.provision_failed",
    "http.lease.release_failed",
    "http.runtime.activation_failed",
    "http.quota.plan_load_failed",
    "http.quota.reserve_failed",
    "http.task.bind_failed",
    "http.task.requeue_failed",
    "http.task.reconcile_failed",
    "http.task.execution_failed",
    "http.task.delete_expired_failed",
    "http.app_session.bind_failed",
    "http.registry.missing_namespace_binding",
    "http.registry.orphan_namespace",
    "http.config.bind_unspecified",
];

/// Writes one line to a sink, best-effort. Logging must never panic or
/// propagate I/O failures (a broken sink should not take down callers).
fn write_line<W: Write>(writer: &mut W, line: &str) {
    let _ = writer.write_all(line.as_bytes());
    let _ = writer.write_all(b"\n");
    let _ = writer.flush();
}

/// Tracks repeated warning occurrences for deduplication.
#[derive(Default)]
struct WarnTracker {
    counts: Mutex<HashMap<String, u64>>,
}

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
/// Calling this a second time returns `Err` with `ErrorKind::AlreadyExists`
/// (the first installation wins) without touching the filesystem.
pub fn install_log_file(path: &str) -> Result<(), io::Error> {
    // Check before opening so a second call never creates a stray file.
    if LOG_FILE_SINK.get().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "log file sink already installed",
        ));
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    LOG_FILE_SINK.set(Mutex::new(file)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "log file sink already installed",
        )
    })
}

/// Logger that writes structured events to stderr (or to the file sink
/// installed via [`install_log_file`]).
#[derive(Clone)]
pub struct StdoutLogger {
    level: LogLevel,
    /// Per-subsystem levels from `RUST_LOG` directives, in the order written.
    /// [`is_event_enabled`] picks the most specific match, so order does not
    /// decide precedence.
    overrides: Vec<(String, LogLevel)>,
    warn_tracker: std::sync::Arc<WarnTracker>,
}

/// The default level from a directive list: the first segment that names a bare
/// level, ignoring `prefix=level` segments.
///
/// A list may lead with a directive rather than a level (`oidc=debug,info`),
/// and taking the whole string would parse it as an unknown level and fall
/// back to `info` by accident rather than by choice.
fn first_directive(configured: &str) -> &str {
    configured
        .split(',')
        .map(str::trim)
        .find(|directive| !directive.is_empty() && !directive.contains('='))
        .unwrap_or("")
}

impl StdoutLogger {
    /// The environment variable that sets how much this process reports.
    pub const LEVEL_ENV: &'static str = "RUST_LOG";

    /// Creates a logger at the level `RUST_LOG` names.
    ///
    /// Every binary builds its logger here, so the documented setting has one
    /// place it is read and no entry point can quietly bypass it. An unset or
    /// unparseable value leaves the logger at `info`: a deployment that cannot
    /// be diagnosed because it said nothing is worse than one that is slightly
    /// too chatty.
    ///
    /// The value is a comma-separated list of directives: a bare level sets the
    /// default, and `prefix=level` sets it for the events whose `op` starts
    /// with `prefix` at a segment boundary. The most specific prefix wins, so
    /// `oidc=error,oidc.callback_rejected=debug` quiets sign-ins except the
    /// one branch.
    #[must_use]
    pub fn from_env() -> Self {
        // A test override wins over the environment, so a test can observe an
        // event the deployment's level filters out without touching the
        // process environment. The `cfg!` keeps the override out of a
        // production build's behaviour: it is read, and only a test can set it.
        #[cfg(test)]
        if let Some(level) = capture::override_level() {
            return Self::from_directives(&level);
        }
        Self::from_env_with(|key| std::env::var(key).ok())
    }

    /// [`from_env`] with the environment read through `lookup`, so the
    /// behaviour can be exercised without mutating the process environment,
    /// which is global state the parallel test harness shares.
    ///
    /// A logger built this way is **bound** to `lookup`: it never consults the
    /// process-global level override.
    ///
    /// That distinction is not cosmetic. A test that supplies its own
    /// directives and then calls production code — which builds its logger
    /// from the environment — is asserting against production behaviour, and
    /// the level that code sees must be the one the test configured. Before
    /// this, `from_env_with` returned a logger whose `is_event_enabled` was
    /// still decided by `from_env`'s override check, so a parallel test in
    /// `with_level` could change this test's result: it failed roughly one run
    /// in eight, in a test that was correct.
    #[must_use]
    pub fn from_env_with<F>(lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let configured = lookup(Self::LEVEL_ENV).unwrap_or_default();
        Self::from_directives(&configured)
    }

    /// Build a logger from a directive list.
    fn from_directives(configured: &str) -> Self {
        let mut logger = Self::new(first_directive(configured));
        // Every segment is examined, not every one after the first: a list may
        // lead with a directive (`oidc=debug,info`), and skipping the first
        // would silently drop the only rule in it.
        for directive in configured.split(',') {
            if let Some((prefix, level)) = directive.split_once('=') {
                let prefix = prefix.trim();
                if prefix.is_empty() {
                    continue;
                }
                // The same prefix twice is one directive written twice, so the
                // later replaces the earlier. Replacing rather than appending
                // means precedence cannot depend on written order, which is the
                // invariant `is_event_enabled`'s tie-break would otherwise
                // break.
                logger.remove_override(prefix);
                logger
                    .overrides
                    .push((prefix.to_string(), LogLevel::parse(level)));
            }
        }
        logger
    }

    fn remove_override(&mut self, prefix: &str) {
        self.overrides.retain(|(existing, _)| existing != prefix);
    }

    /// Creates a new logger with the specified minimum log level.
    #[must_use]
    pub fn new(level: &str) -> Self {
        Self {
            level: LogLevel::parse(level),
            overrides: Vec::new(),
            warn_tracker: std::sync::Arc::new(WarnTracker::default()),
        }
    }

    /// The level this logger emits at and above, for events no directive names.
    #[must_use]
    pub fn level(&self) -> LogLevel {
        self.level
    }

    /// Whether an event with this `op` and level would be emitted.
    ///
    /// The most specific matching directive decides, so a broad one can be
    /// narrowed without repeating it. An event with no `op` uses the default
    /// level: it names no subsystem, so no subsystem rule can be about it.
    #[must_use]
    pub fn is_event_enabled(&self, level: LogLevel, op: &str) -> bool {
        let applicable = self
            .overrides
            .iter()
            .filter(|(prefix, _)| op == prefix || op.starts_with(&format!("{prefix}.")))
            .max_by_key(|(prefix, _)| prefix.len());
        let threshold = applicable.map_or(self.level, |(_, level)| *level);
        level >= threshold
    }

    /// Logs a warning with deduplication. The `dedup_key` identifies
    /// repeated occurrences. The first occurrence is always logged.
    /// Subsequent occurrences are logged only at every Nth repetition
    /// (controlled by `every_nth`, default 10).
    pub fn log_warn_dedup(&self, event: HashMap<String, Value>, dedup_key: &str, every_nth: u64) {
        let count = {
            // Logging must never panic on a poisoned counter; the count map
            // remains valid to increment after recovery.
            let mut counts = self
                .warn_tracker
                .counts
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let c = counts.entry(dedup_key.to_string()).or_insert(0);
            *c += 1;
            *c
        };

        if count == 1 || count % every_nth == 0 {
            let mut event = event;
            if count > 1 {
                event.insert("repeat_count".to_string(), Value::Number(count.into()));
            }
            self.log(event, LogLevel::Warn);
        }
    }

    /// Returns true if the provided `level` should be emitted given the
    /// currently configured minimum level.
    #[must_use]
    pub fn is_enabled(&self, level: LogLevel) -> bool {
        level >= self.level
    }

    /// Logs an event if the level is enabled.
    ///
    /// The logger respects the configured minimum `level`. Messages with a
    /// severity lower than the configured level are dropped. `debug` and
    /// `trace` messages are emitted only when the logger is configured to
    /// `debug`/`trace` respectively (no global unconditional suppression).
    ///
    /// An event carrying an `op` is filtered by its own subsystem's level, so
    /// a directive can turn one subsystem up without turning up the hot paths
    /// that would drown it.
    pub fn log(&self, event: HashMap<String, Value>, level: LogLevel) {
        let op = event.get("op").and_then(Value::as_str);
        let enabled = match op {
            Some(op) => self.is_event_enabled(level, op),
            None => level >= self.level,
        };
        if !enabled {
            return;
        }

        let line = Self::format_event_line(&event, level);

        // Write to file sink if installed; otherwise fall through to stderr.
        if let Some(sink) = LOG_FILE_SINK.get() {
            let mut file = sink.lock().unwrap_or_else(|poison| poison.into_inner());
            write_line(&mut *file, &line);
            return;
        }

        // A test observing the rendered line reads it here. This is compiled
        // only under `cfg(test)`, so production output is unchanged.
        #[cfg(test)]
        capture::record(&line);

        let mut stderr = io::stderr();
        write_line(&mut stderr, &line);
    }

    /// Formats an event into a single human-readable line.
    #[must_use]
    pub fn format_event_line(event: &HashMap<String, Value>, level: LogLevel) -> String {
        let ts = Utc::now().to_rfc3339();
        Self::format_event_line_with_ts(event, level, &ts)
    }

    /// Formats an event with a provided timestamp.
    pub(crate) fn format_event_line_with_ts(
        event: &HashMap<String, Value>,
        level: LogLevel,
        ts: &str,
    ) -> String {
        // Truncate timestamp to milliseconds for readability: ...608.123456Z -> ...608Z
        let ts_short = if ts.len() > 23 {
            // RFC3339: "2026-04-12T20:03:59.608616+00:00" → find '.' then keep 3 digits then 'Z'
            if let Some(dot) = ts.find('.') {
                format!("{}Z", &ts[..dot + 4])
            } else {
                ts.to_string()
            }
        } else {
            ts.to_string()
        };

        // Extract special fields for prominent placement
        let request_id = event
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        // A duration may be fractional. `as_u64` would drop the fraction, and
        // a sub-millisecond stage — the ones worth timing, since a fast cache
        // hit and a query are otherwise indistinguishable — would read as
        // zero, the same as a stage that was never measured.
        let duration_ms = event.get("duration_ms").and_then(render_duration);

        let mut parts = Vec::with_capacity(event.len() + 4);
        // Header: [ts] LEVEL  req=XXXX
        parts.push(format!(
            "[{}] {:<5} req={:<6}",
            ts_short,
            level.as_str().to_uppercase(),
            request_id
        ));

        // Build remaining keys, excluding special fields we already rendered
        let special_keys = ["request_id", "duration_ms"];
        let mut keys: Vec<_> = event
            .keys()
            .filter(|k| !special_keys.contains(&k.as_str()))
            .cloned()
            .collect();
        keys.sort();

        // Render: op first, then duration_ms (if present), then the rest
        if let Some(pos) = keys.iter().position(|k| k == "op") {
            let op = keys.remove(pos);
            if let Some(value) = event.get(&op) {
                parts.push(format!("{}={}", op, value_to_string(value)));
            }
        }

        if let Some(ms) = duration_ms {
            parts.push(format!("duration_ms={}", ms));
        }

        for key in keys {
            if let Some(value) = event.get(&key) {
                let value_str = value_to_string(value);
                parts.push(format!("{}={}", key, quote_if_needed(&value_str)));
            }
        }

        parts.join("  ")
    }
}

/// Converts a JSON value to a string representation.
///
/// Objects are flattened to key=value pairs, arrays to comma-separated lists.
/// Long values are truncated to MAX_LEN characters.
///
/// Special handling for Rust artifacts:
/// - `Some(value)` → extracts inner value
/// - `None` → "null"
/// - `Ok(value)` → extracts inner value
/// - `Err(value)` → formats as "Err(error_msg)"
fn value_to_string(value: &Value) -> String {
    const MAX_LEN: usize = 200;

    let s = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(arr) => {
            let elems: Vec<String> = arr.iter().map(value_to_string).collect();
            format!("[{}]", elems.join(","))
        }
        Value::Object(map) => {
            // Handle Rust Option/Result artifacts from serde
            if let Some(Value::String(inner)) = map.get("Some") {
                return inner.clone();
            }
            if let Some(inner) = map.get("Some") {
                return value_to_string(inner);
            }
            if map.contains_key("None") {
                return "null".to_string();
            }
            if let Some(Value::String(inner)) = map.get("Ok") {
                return inner.clone();
            }
            if let Some(inner) = map.get("Ok") {
                return value_to_string(inner);
            }
            if let Some(Value::String(err)) = map.get("Err") {
                return format!("Err({})", err);
            }
            if let Some(inner) = map.get("Err") {
                return format!("Err({})", value_to_string(inner));
            }

            let mut pairs = Vec::with_capacity(map.len());
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            for k in keys {
                if let Some(v) = map.get(&k) {
                    pairs.push(format!("{}={}", k, value_to_string(v)));
                }
            }
            format!("{{{}}}", pairs.join(","))
        }
    };

    if s.len() > MAX_LEN {
        let end = s
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|&index| index <= MAX_LEN - 3)
            .last()
            .unwrap_or(0);
        format!("{}...", &s[..end])
    } else {
        s
    }
}

/// Quotes a string if it contains special characters.
fn quote_if_needed(s: &str) -> String {
    if s.contains(char::is_whitespace) || s.contains('=') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "'"))
    } else {
        s.to_string()
    }
}

/// Render a `duration_ms` value for the line.
///
/// Whole numbers keep their integral form so an existing whole-millisecond line
/// reads the same as before; a fractional one keeps its fraction, which is the
/// only part that distinguishes a sub-millisecond stage from an unmeasured one.
fn render_duration(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => {
            if let Some(whole) = number.as_u64() {
                Some(whole.to_string())
            } else {
                number.as_f64().map(|fractional| format!("{fractional}"))
            }
        }
        _ => None,
    }
}

/// Capture what the logger writes, for a test that has to observe a line
/// rather than an event.
///
/// The logger writes to stderr or to the process-wide file sink, neither of
/// which a test can read back. This records every rendered line in a buffer
/// while a capture is installed, so a test can assert on the line an operator
/// actually reads — which is where a formatting bug is invisible when the
/// assertion is made against a structure instead.
///
/// Each guard owns its buffer. An earlier version shared one and counted
/// holders, which made two tests running in parallel see each other's lines and
/// made a two-phase assertion in one test read the first phase twice. Here the
/// installed buffer is swapped per guard, so a test reads only what it caused —
/// at the cost of a test that captures twice needing to read the first
/// capture's lines before installing the second.
#[cfg(test)]
pub mod capture {
    use std::sync::{Arc, Mutex, OnceLock};

    /// One test's captured lines, shared between the guard that owns it and
    /// the logger that writes into it.
    type Buffer = Arc<Mutex<Vec<String>>>;

    /// Every buffer currently capturing.
    ///
    /// A list, not one slot: the harness runs tests in parallel, and a single
    /// slot meant a test that installed a capture while another was running
    /// silently stole its output — the other asserted on an empty buffer and
    /// failed for a reason that had nothing to do with its own code. Every live
    /// capture receives every line, so a parallel test cannot take another's
    /// output away. A test asserts only on what it can attribute to itself,
    /// which is what `clear` is for between phases of one test.
    static ACTIVE: OnceLock<Mutex<Vec<Buffer>>> = OnceLock::new();

    /// Ends the capture when dropped.
    pub struct CaptureGuard {
        lines: Buffer,
    }

    impl CaptureGuard {
        /// The lines written since this guard was installed.
        #[must_use]
        pub fn lines(&self) -> Vec<String> {
            self.lines
                .lock()
                .map(|lines| lines.clone())
                .unwrap_or_default()
        }

        /// Forget the lines written so far, keeping the capture installed.
        ///
        /// For a test that checks two phases of one event: the second phase
        /// must not read the first one's lines.
        pub fn clear(&self) {
            self.lines
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clear();
        }
    }

    impl Drop for CaptureGuard {
        fn drop(&mut self) {
            let id = Arc::as_ptr(&self.lines);
            if let Some(slot) = ACTIVE.get()
                && let Ok(mut active) = slot.lock()
            {
                active.retain(|buffer| Arc::as_ptr(buffer) != id);
            }
        }
    }

    /// Starts capturing log output into a buffer of its own.
    pub fn install() -> CaptureGuard {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let slot = ACTIVE.get_or_init(|| Mutex::new(Vec::new()));
        if let Ok(mut active) = slot.lock() {
            active.push(lines.clone());
        }
        CaptureGuard { lines }
    }

    /// Record a rendered line into every buffer currently capturing.
    pub(crate) fn record(line: &str) {
        let Some(slot) = ACTIVE.get() else { return };
        let Ok(active) = slot.lock() else { return };
        for buffer in active.iter() {
            if let Ok(mut lines) = buffer.lock() {
                lines.push(line.to_owned());
            }
        }
    }

    /// Run `body` with a level override in effect, then restore.
    ///
    /// The logger reads `RUST_LOG` per call rather than caching it, so this is
    /// what lets a test observe an event that the default level filters out
    /// without mutating the process environment — which is global state the
    /// parallel test harness shares, and which other tests read.
    ///
    /// Overrides are a stack, not a single slot. A single slot meant one test's
    /// `with_level` overwrote another's, and the test that lost it saw the
    /// deployment's level instead of its own — which is how a lease-scheduler
    /// test asserting on its own log line failed intermittently while passing
    /// on its own. Last override wins, and it is restored on the way out
    /// including on unwind.
    pub async fn with_level<F, Fut, T>(level: &str, body: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _guard = LevelOverride::install(level);
        body().await
    }

    /// Restores the previous override when dropped, including on unwind.
    ///
    /// The guard remembers *what it pushed* and removes that entry, rather than
    /// popping the top of the stack. Popping the top is only correct when
    /// pushes and drops are strictly nested, and under a parallel test
    /// harness they are not: two tests can install concurrently and drop in the
    /// opposite order, and the second drop then removes the first test's level.
    /// The stack is left holding the wrong directive, and a test that passed its
    /// own `http=error` sees `http=info` instead — which is how a correct test
    /// failed about one run in ninety.
    ///
    /// Removing by value rather than by position also means an override that
    /// was never pushed (a poisoned lock, a failed install) cannot remove
    /// someone else's.
    pub(crate) struct LevelOverride {
        slot: &'static Mutex<Vec<String>>,
        level: String,
        pushed: bool,
    }

    impl LevelOverride {
        pub(crate) fn install(level: &str) -> Self {
            let slot = OVERRIDE_LEVEL.get_or_init(|| Mutex::new(Vec::new()));
            let pushed = slot
                .lock()
                .map(|mut current| {
                    current.push(level.to_string());
                })
                .is_ok();
            LevelOverride {
                slot,
                level: level.to_string(),
                pushed,
            }
        }
    }

    impl Drop for LevelOverride {
        fn drop(&mut self) {
            if !self.pushed {
                return;
            }
            if let Ok(mut current) = self.slot.lock()
                && let Some(position) = current.iter().rposition(|held| *held == self.level)
            {
                current.remove(position);
            }
        }
    }

    /// The level a test has in force, if any, taking precedence over
    /// `RUST_LOG`.
    pub(crate) fn override_level() -> Option<String> {
        OVERRIDE_LEVEL
            .get()
            .and_then(|slot| slot.lock().ok().and_then(|stack| stack.last().cloned()))
    }

    /// A stack, not a single slot: overrides nest, and the innermost one is
    /// the level in force. See `with_level`.
    static OVERRIDE_LEVEL: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Serialises the two tests that assert on the override stack's absolute
    /// contents. It is process-global, so they would otherwise interleave and
    /// each would see the other's directive.
    fn override_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    /// An override must be removed by identity, not by position.
    ///
    /// The guard used to `pop()` the top of the stack, which is only right when
    /// pushes and drops are strictly nested. They are not under a parallel test
    /// harness: two tests install concurrently and drop in the opposite order,
    /// and the second drop then removes the *first* test's directive. The stack
    /// is left holding the wrong level, and a test that installed its own
    /// `http=error` reads `http=info` — a correct test failing about one run in
    /// ninety, on the one signal the suite uses to prove the filter works.
    ///
    /// Driven here in the order that breaks it: the outer override outlives the
    /// inner one, so a positional pop removes the wrong entry and the outer
    /// test is left seeing the inner one's level.
    #[test]
    fn an_override_is_removed_by_identity_not_by_position() {
        let _held = override_lock();
        // Distinct directives per test: the stack is process-global and the
        // harness runs tests in parallel, so two tests sharing a value would
        // remove each other's entry and both would be wrong.
        let outer = capture::LevelOverride::install("probe.a=info");
        assert_eq!(capture::override_level().as_deref(), Some("probe.a=info"));

        // The inner guard is dropped first, while the outer is still in force.
        {
            let _inner = capture::LevelOverride::install("probe.a=error");
            assert_eq!(
                capture::override_level().as_deref(),
                Some("probe.a=error"),
                "the innermost override is the one in force"
            );
        }

        // A positional pop would have removed `probe.a=error` here and left
        // `probe.a=info` on the stack, which is right by luck. The failure is
        // the reverse order: an override outliving one nested inside it.
        drop(outer);
        assert_eq!(
            capture::override_level(),
            None,
            "both overrides must be gone; a pop() that removed the wrong entry \
             would leave one behind and every later test would read it"
        );
    }

    /// The order that actually broke: an override installed *outside* another
    /// that is dropped last.
    #[test]
    fn an_outer_override_outliving_an_inner_one_is_restored_correctly() {
        let _held = override_lock();
        let inner_first = capture::LevelOverride::install("probe.b=error");
        // Installed second, so a positional pop on its drop would take
        // `probe.b=error` and leave `probe.b=info` behind.
        let second = capture::LevelOverride::install("probe.b=info");
        assert_eq!(capture::override_level().as_deref(), Some("probe.b=info"));

        drop(inner_first);
        assert_eq!(
            capture::override_level().as_deref(),
            Some("probe.b=info"),
            "dropping an override that is not on top must not disturb the one \
             that is; a pop() removes the top regardless, which left the wrong \
             directive in force for whichever test ran next"
        );

        drop(second);
        assert_eq!(capture::override_level(), None);
    }

    #[test]
    fn log_level_parse_recognizes_valid_levels() {
        assert_eq!(LogLevel::parse("trace"), LogLevel::Trace);
        assert_eq!(LogLevel::parse("DEBUG"), LogLevel::Debug);
        assert_eq!(LogLevel::parse("info"), LogLevel::Info);
        assert_eq!(LogLevel::parse("WARN"), LogLevel::Warn);
        assert_eq!(LogLevel::parse("warning"), LogLevel::Warn);
        assert_eq!(LogLevel::parse("error"), LogLevel::Error);
    }

    #[test]
    fn log_level_parse_defaults_to_info() {
        assert_eq!(LogLevel::parse("unknown"), LogLevel::Info);
        assert_eq!(LogLevel::parse(""), LogLevel::Info);
    }

    #[test]
    fn log_level_as_str() {
        assert_eq!(LogLevel::Trace.as_str(), "trace");
        assert_eq!(LogLevel::Debug.as_str(), "debug");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Error.as_str(), "error");
    }

    #[test]
    fn format_simple_event_contains_keys() {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("migrations"));
        event.insert("stage".to_string(), json!("start"));
        event.insert("source".to_string(), json!("filesystem"));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-01-01T00:00:00.000+00:00",
        );

        assert!(line.contains("[2026-01-01T00:00:00.000Z] INFO "));
        assert!(line.contains("req=-"));
        assert!(line.contains("op=migrations"));
        assert!(line.contains("stage=start"));
        assert!(line.contains("source=filesystem"));
    }

    #[test]
    fn format_object_and_array_and_quoting() {
        let mut event = HashMap::new();
        event.insert("name".to_string(), json!("Dmitry Ivanov"));
        event.insert("list".to_string(), json!(["a", "b", "c"]));
        event.insert("args".to_string(), json!({"scope": "org", "query": "ARR"}));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-01-01T00:00:00+00:00",
        );

        assert!(line.contains("name=\"Dmitry Ivanov\""));
        assert!(line.contains("list=[a,b,c]"));
        assert!(line.contains("args="));
        assert!(line.contains("query=ARR"));
        assert!(line.contains("scope=org"));
    }

    #[test]
    fn format_truncates_long_values() {
        let long = "x".repeat(300);
        let mut event = HashMap::new();
        event.insert("long".to_string(), json!(long));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-01-01T00:00:00+00:00",
        );

        assert!(line.contains("..."));

        if let Some(pos) = line.find("long=") {
            let rest = &line[pos + 5..];
            let value = rest.split_whitespace().next().unwrap_or("");
            assert_eq!(value.len(), 200);
        } else {
            panic!("missing long=");
        }
    }

    #[test]
    fn value_to_string_truncates_multibyte_utf8_without_panic() {
        // 10 repetitions of a 31-char Cyrillic string = 310 bytes (each char is 2 bytes).
        // Byte index 197 is guaranteed to land mid-character.
        let long_cyrillic = "абвгдежзийклмнопрстуфхцчшщъыьэюя".repeat(10);
        assert!(long_cyrillic.len() > 200);
        let result = value_to_string(&json!(long_cyrillic));
        assert!(result.ends_with("..."));
        // Must not panic — the truncation uses floor_char_boundary
    }

    #[test]
    fn format_event_line_uses_current_timestamp() {
        let event = HashMap::new();
        let line = StdoutLogger::format_event_line(&event, LogLevel::Info);
        assert!(line.contains("] INFO"));
    }

    #[test]
    fn format_with_request_id_and_duration() {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("extract.done"));
        event.insert("request_id".to_string(), json!("req_0042"));
        event.insert("duration_ms".to_string(), json!(152u64));
        event.insert("entities".to_string(), json!(3u64));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-04-12T20:03:59.608616+00:00",
        );

        assert!(line.contains("[2026-04-12T20:03:59.608Z]"));
        assert!(line.contains("INFO "));
        assert!(line.contains("req=req_0042"));
        assert!(line.contains("op=extract.done"));
        assert!(line.contains("duration_ms=152"));
        assert!(line.contains("entities=3"));
        // request_id should NOT appear again in the key-value section
        let after_op = line.split("op=extract.done").nth(1).unwrap_or("");
        assert!(!after_op.contains("request_id="));
    }

    #[test]
    fn format_without_request_id_shows_dash() {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("main.startup"));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-04-12T20:03:59.608616+00:00",
        );

        assert!(line.contains("req=-"));
    }

    #[test]
    fn is_enabled_respects_configured_level() {
        let info_logger = StdoutLogger::new("info");
        assert!(info_logger.is_enabled(LogLevel::Info));
        assert!(!info_logger.is_enabled(LogLevel::Debug));
        assert!(!info_logger.is_enabled(LogLevel::Trace));

        let debug_logger = StdoutLogger::new("debug");
        assert!(debug_logger.is_enabled(LogLevel::Debug));
        assert!(!debug_logger.is_enabled(LogLevel::Trace));
        assert!(debug_logger.is_enabled(LogLevel::Info));

        let trace_logger = StdoutLogger::new("trace");
        assert!(trace_logger.is_enabled(LogLevel::Trace));
        assert!(trace_logger.is_enabled(LogLevel::Debug));
        assert!(trace_logger.is_enabled(LogLevel::Info));
    }

    #[test]
    fn value_to_string_handles_option_some() {
        let some_value = json!({"Some": "hello"});
        assert_eq!(value_to_string(&some_value), "hello");

        // Nested SurrealDB-style Some with String wrapper
        let some_nested = json!({"Some": {"String": "world"}});
        // This extracts the inner object which is {String=world}
        assert_eq!(value_to_string(&some_nested), "{String=world}");
    }

    #[test]
    fn value_to_string_handles_option_none() {
        let none_value = json!({"None": null});
        assert_eq!(value_to_string(&none_value), "null");
    }

    #[test]
    fn value_to_string_handles_result_ok() {
        let ok_value = json!({"Ok": "success"});
        assert_eq!(value_to_string(&ok_value), "success");

        let ok_nested = json!({"Ok": {"value": 42}});
        assert_eq!(value_to_string(&ok_nested), "{value=42}");
    }

    #[test]
    fn value_to_string_handles_result_err() {
        let err_value = json!({"Err": "not found"});
        assert_eq!(value_to_string(&err_value), "Err(not found)");

        let err_nested = json!({"Err": {"code": 404}});
        assert_eq!(value_to_string(&err_nested), "Err({code=404})");
    }

    /// `RUST_LOG` is the deployment's only dial for how much it says, and the
    /// README documents it as taking effect. It used not to: the HTTP binary
    /// built its logger from a literal `"info"`, so an operator who set
    /// `RUST_LOG=error` to quiet a deployment was ignored, and one who set
    /// `RUST_LOG=debug` to see more saw nothing extra. Reading the variable
    /// here — once, in the constructor every binary goes through — is what
    /// makes the documented setting real.
    #[test]
    fn the_configured_level_comes_from_the_environment() {
        let logger = StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("warn".to_string()),
            _ => None,
        });
        assert!(
            !logger.is_enabled(LogLevel::Info),
            "info must be below warn"
        );
        assert!(logger.is_enabled(LogLevel::Warn));
    }

    /// An unset or unreadable variable must not silence the process. A
    /// deployment that cannot be diagnosed because its log level defaulted to
    /// nothing is worse than one that is slightly too chatty.
    #[test]
    fn an_unset_level_falls_back_to_info() {
        for absent in [
            |_key: &str| None,
            |_key: &str| Some("nonsense".to_string()),
            |_key: &str| Some(String::new()),
        ] {
            let logger = StdoutLogger::from_env_with(absent);
            assert!(
                logger.is_enabled(LogLevel::Info),
                "a deployment with no usable RUST_LOG must still report info"
            );
        }
    }

    /// One level for the whole process means a subsystem that needs detail —
    /// an identity provider callback, a cache — cannot be turned up without
    /// turning up everything, including the hot paths. Turning those up to
    /// diagnose one sign-in is how a deployment ends up logging at `trace` and
    /// filling its disk.
    ///
    /// The selector is the `op` prefix, which every event already carries
    /// (`ner.artifact_refresh.failed`, `oidc.callback_rejected`), so nothing
    /// has to be annotated to become filterable.
    #[test]
    fn a_subsystem_can_be_turned_up_without_turning_up_the_rest() {
        let logger = StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("info,oidc=debug,ner=warn".to_string()),
            _ => None,
        });

        assert!(
            logger.is_event_enabled(LogLevel::Debug, "oidc.callback_rejected"),
            "the named subsystem takes its own level"
        );
        assert!(
            !logger.is_event_enabled(LogLevel::Debug, "ner.extract.done"),
            "a subsystem named at a lower level must not inherit the global one"
        );
        assert!(
            logger.is_event_enabled(LogLevel::Info, "http.request"),
            "an unnamed subsystem keeps the global level"
        );
        assert!(
            !logger.is_event_enabled(LogLevel::Debug, "http.request"),
            "an unnamed subsystem must not inherit a named subsystem's level"
        );
    }

    /// A subsystem name matches whole segments. `ner` selects `ner.extract` and
    /// not a subsystem whose name merely starts the same way: a bare prefix
    /// match would quietly capture events from a subsystem nobody named, and
    /// the level meant for one would then apply to the other.
    ///
    /// `nerdb` is given no directive of its own, so the only thing that can
    /// let a `trace` event through is `ner` matching a name it does not own.
    #[test]
    fn a_subsystem_name_matches_whole_segments() {
        let logger = StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("error,ner=trace".to_string()),
            _ => None,
        });

        assert!(logger.is_event_enabled(LogLevel::Trace, "ner.extract.done"));
        assert!(
            !logger.is_event_enabled(LogLevel::Trace, "nerdb.query.run"),
            "`ner` must not select `nerdb`: the segments differ"
        );
    }

    /// The most specific rule wins, so a specific directive can override a
    /// broad one: `oidc=error,oidc.callback=debug` is the difference between
    /// silencing sign-ins and reading them.
    #[test]
    fn the_most_specific_rule_wins() {
        let logger = StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("error,oidc=error,oidc.callback_rejected=debug".to_string()),
            _ => None,
        });

        assert!(logger.is_event_enabled(LogLevel::Debug, "oidc.callback_rejected"));
        assert!(
            !logger.is_event_enabled(LogLevel::Info, "oidc.authorize"),
            "a sibling op must keep the broader rule"
        );
    }

    /// A malformed directive must not take the rest of the string down with it:
    /// an operator who mistypes one segment still needs the others honoured.
    #[test]
    fn a_malformed_directive_does_not_disable_the_rest() {
        let logger = StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("info,=nonsense,oidc=debug,,=also-nonsense".to_string()),
            _ => None,
        });

        assert!(
            logger.is_event_enabled(LogLevel::Debug, "oidc.callback_rejected"),
            "a valid directive after a malformed one still applies"
        );
        assert!(logger.is_event_enabled(LogLevel::Info, "http.request"));
    }

    /// Sub-millisecond steps are the ones worth timing. The stages of an
    /// identity callback — unseal, exchange, validate — are often under a
    /// millisecond, and truncating each to `0ms` reports every one of them as
    /// zero, which is indistinguishable from a stage that was never measured.
    /// That is exactly the signal needed to tell a cache hit from a query.
    #[test]
    fn a_sub_millisecond_duration_is_still_reported() {
        let event = serde_json::json!({
            "op": "oidc.callback_stage",
            "stage": "unseal",
            "duration_ms": 0.42,
        });
        let event: std::collections::HashMap<String, Value> = event
            .as_object()
            .expect("object")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let line = StdoutLogger::format_event_line(&event, LogLevel::Debug);

        assert!(
            line.contains("duration_ms=0.42"),
            "a sub-millisecond step must not be reported as zero: {line}"
        );
    }

    /// A duration given as a whole number still renders as a whole number:
    /// the sub-millisecond support must not add a decimal to every line.
    #[test]
    fn a_whole_duration_is_unchanged() {
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), Value::from("oidc.callback_stage"));
        event.insert("duration_ms".to_string(), Value::from(271u64));

        let line = StdoutLogger::format_event_line(&event, LogLevel::Info);

        assert!(line.contains("duration_ms=271"), "{line}");
    }
}
