//! Structured logging utilities.
//!
//! A bespoke `StdoutLogger` records structured events, filtering them by
//! `RUST_LOG` before emission, and hands each to a process-wide `tracing`
//! subscriber, which formats and writes it. Events go to **stderr** (or to the
//! file sink installed at startup), never to stdout: under the stdio transport
//! stdout is reserved for MCP protocol framing.
//!
//! The subscriber is installed once (see [`install`]); `MEMORY_LOG_FORMAT`
//! selects human-readable text (default) or NDJSON, and colour is enabled only
//! for a colour-capable terminal unless `MEMORY_LOG_COLOR` says otherwise. See
//! [ADR-0078](../../../docs/adr/0078-human-readable-logging-on-tracing.md).

use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::sync::{Mutex, OnceLock};

use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::{FmtContext, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;

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

/// How the logger decides whether to emit ANSI colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorMode {
    /// Colour only when the destination is a colour-capable terminal.
    Auto,
    /// Colour even without a TTY (an explicit operator override).
    Always,
    /// Never colour.
    Never,
}

impl ColorMode {
    /// Parses `MEMORY_LOG_COLOR`. Unknown values fall back to `Auto`, so a
    /// typo cannot silently disable colour for an interactive operator.
    #[must_use]
    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "always" => Self::Always,
            "never" => Self::Never,
            _ => Self::Auto,
        }
    }
}

/// Whether output should carry ANSI colour, from the destination and the
/// environment.
///
/// Pure on purpose: the truth table is exercisable without a terminal and
/// without installing a process-global subscriber. `sink` is true when
/// `MEMORY_LOG_FILE` is installed — a file is never a terminal, so escape
/// codes there are corruption, not colour. `no_color` is `NO_COLOR` set and
/// non-empty; `term` is `TERM` (`"dumb"` is not colour-capable); `mode` is
/// `MEMORY_LOG_COLOR`.
#[must_use]
pub(crate) fn use_ansi(
    sink: bool,
    is_tty: bool,
    no_color: bool,
    term: &str,
    mode: ColorMode,
) -> bool {
    match mode {
        ColorMode::Never => false,
        ColorMode::Always => !sink && !no_color,
        ColorMode::Auto => !sink && !no_color && is_tty && term != "dumb",
    }
}

/// The output encoding, chosen by `MEMORY_LOG_FORMAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogFormat {
    /// The human-readable single line an operator reads.
    Text,
    /// One JSON object per line, for a log collector.
    Json,
}

impl LogFormat {
    /// Parses `MEMORY_LOG_FORMAT`. Unknown values fall back to `Text`.
    #[must_use]
    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Self::Json,
            _ => Self::Text,
        }
    }
}

/// The `tracing` target every recorded event carries.
///
/// It is the one thing the `Targets` filter keys on to tell our events — already
/// filtered by `RUST_LOG` before emission — from third-party noise.
pub(crate) const LOG_TARGET: &str = "memory_mcp";

/// The level `RUST_LOG` is not allowed to reach: third-party events below it
/// are dropped rather than flooding the stream the operator reads.
const FOREIGN_DEFAULT_LEVEL: tracing::Level = tracing::Level::WARN;

fn from_tracing_level(level: &tracing::Level) -> LogLevel {
    match *level {
        tracing::Level::ERROR => LogLevel::Error,
        tracing::Level::WARN => LogLevel::Warn,
        tracing::Level::INFO => LogLevel::Info,
        tracing::Level::DEBUG => LogLevel::Debug,
        tracing::Level::TRACE => LogLevel::Trace,
    }
}

/// Reads a `tracing` event's fields: our events carry the serialised map in a
/// single `payload` field; a foreign event carries its own fields instead.
#[derive(Default)]
struct EventVisitor {
    payload: Option<String>,
    fields: Vec<(String, String)>,
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "payload" {
            self.payload = Some(value.to_string());
        } else {
            self.fields
                .push((field.name().to_string(), value.to_string()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "payload" {
            self.payload = Some(format!("{value:?}"));
        } else {
            self.fields
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }
}

/// Renders every event — ours and foreign — into the configured format.
///
/// A `tracing` field set is static per callsite, but our payload is an arbitrary
/// map, so it travels in one field that this formatter flattens. Foreign events
/// (no `payload`) are rendered from their own fields so a third-party warning is
/// readable instead of dropped.
pub(crate) struct MemoryFormat {
    format: LogFormat,
}

impl<S, N> FormatEvent<S, N> for MemoryFormat
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let level = from_tracing_level(event.metadata().level());
        let ansi = writer.has_ansi_escapes();

        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);

        let line = match visitor.payload {
            Some(payload) => {
                let event: HashMap<String, Value> =
                    serde_json::from_str(&payload).unwrap_or_default();
                match self.format {
                    LogFormat::Text => render_human(&event, level, &ts, ansi),
                    LogFormat::Json => render_json(&event, level, &ts),
                }
            }
            None => render_foreign(
                event.metadata().target(),
                &visitor.fields,
                level,
                &ts,
                ansi,
                self.format,
            ),
        };

        #[cfg(test)]
        capture::record(&line);

        writeln!(writer, "{line}")
    }
}

/// Render a foreign (`tracing`-originated) event in the same shape.
fn render_foreign(
    target: &str,
    fields: &[(String, String)],
    level: LogLevel,
    ts: &str,
    ansi: bool,
    format: LogFormat,
) -> String {
    let message = fields
        .iter()
        .find(|(key, _)| key == "message")
        .map(|(_, value)| value.clone());

    match format {
        LogFormat::Text => {
            let level_text = format!("{:>5}", level.as_str().to_uppercase());
            let level_text = if ansi {
                format!("{}{level_text}{ANSI_RESET}", level_ansi(level))
            } else {
                level_text
            };
            let mut parts = Vec::with_capacity(fields.len());
            if let Some(message) = message {
                parts.push(message);
            }
            for (key, value) in fields {
                if key == "message" {
                    continue;
                }
                parts.push(key_token(key, &quote_if_needed(value), ansi));
            }
            let separator = if parts.is_empty() { "" } else { " " };
            format!("{ts} {level_text} {target}:{separator}{}", parts.join(" "))
        }
        LogFormat::Json => {
            let mut object = serde_json::Map::with_capacity(fields.len() + 3);
            object.insert("timestamp".to_string(), Value::String(ts.to_string()));
            object.insert(
                "level".to_string(),
                Value::String(level.as_str().to_string()),
            );
            object.insert("target".to_string(), Value::String(target.to_string()));
            let mut mapped = serde_json::Map::with_capacity(fields.len());
            for (key, value) in fields {
                mapped.insert(key.clone(), Value::String(value.clone()));
            }
            object.insert("fields".to_string(), Value::Object(mapped));
            Value::Object(object).to_string()
        }
    }
}

/// The `MakeWriter` behind the `fmt` layer: the `MEMORY_LOG_FILE` sink when
/// installed, otherwise stderr.
///
/// Resolved per write, not at install, so a sink installed after the subscriber
/// still takes effect — the same guarantee the old direct-write logger gave.
pub(crate) struct LogMakeWriter;

/// Where one formatted event is written. Errors are swallowed: logging must
/// never panic or take a caller down, and the line is flushed on drop so a file
/// sink is durable per line exactly as before.
pub(crate) enum SinkWriter {
    File(std::sync::MutexGuard<'static, File>),
    Stderr(io::Stderr),
}

impl<'a> MakeWriter<'a> for LogMakeWriter {
    type Writer = SinkWriter;

    fn make_writer(&'a self) -> Self::Writer {
        match LOG_FILE_SINK.get() {
            Some(sink) => {
                SinkWriter::File(sink.lock().unwrap_or_else(|poison| poison.into_inner()))
            }
            None => SinkWriter::Stderr(io::stderr()),
        }
    }
}

impl Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::File(file) => file.write(buf),
            Self::Stderr(stderr) => stderr.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::File(file) => file.flush(),
            Self::Stderr(stderr) => stderr.flush(),
        }
    }
}

impl Drop for SinkWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// Whether colour is enabled, from the destination and the environment.
fn ansi_enabled_from_env() -> bool {
    let sink = LOG_FILE_SINK.get().is_some();
    let is_tty = io::stderr().is_terminal();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let term = std::env::var("TERM").unwrap_or_default();
    let mode = ColorMode::parse(&std::env::var("MEMORY_LOG_COLOR").unwrap_or_default());
    use_ansi(sink, is_tty, no_color, &term, mode)
}

/// Install the process-wide `tracing` subscriber, once.
///
/// Idempotent: the first caller wins and later calls are no-ops, so both
/// binaries can call it explicitly and a library or test that logs without
/// doing so still gets a working pipeline. Errors from
/// `set_global_default` (a subscriber already set by something else) are
/// ignored — logging must never fail a caller.
pub fn install() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let format = LogFormat::parse(&std::env::var("MEMORY_LOG_FORMAT").unwrap_or_default());
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(MemoryFormat { format })
                    .with_writer(LogMakeWriter)
                    .with_ansi(ansi_enabled_from_env()),
            )
            .with(
                Targets::new()
                    .with_target(LOG_TARGET, tracing::Level::TRACE)
                    .with_default(FOREIGN_DEFAULT_LEVEL),
            );
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
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
    "http.runtime.binding_conflict",
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
    // Emitted from the runtime/bootstrap, runtime/storage and
    // embedding/backfill_scheduler sources. They were absent here while the
    // inventory scanner did not read those files, so the completeness test
    // passed on a list that did not cover them.
    "http.start",
    "http.embedding_policy_resolved",
    "http.entity_extractor_unavailable",
    "http.embedding.backfill_bind_failed",
    "http.embedding.backfill_completed",
    "http.embedding.backfill_declined",
    "http.embedding.backfill_disabled",
    "http.embedding.backfill_failed",
    "http.tenant_embedding_decision",
    "http.tenant_embedding_read_failed",
    "http.tenant_embedding_index_reconcile_skipped",
    "http.tenant_embedding_index_redefined",
    "http.tenant_embedding_index_reconcile_failed",
    "http.lifecycle.decay_failed",
    "http.lifecycle.archival_failed",
];

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

/// The path `MEMORY_LOG_FILE` names, or `None` when the variable is unset or
/// blank. A blank value means "unset", not "write to a file called nothing".
fn resolve_log_file_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Install the file sink named by `MEMORY_LOG_FILE`, if the variable is set.
///
/// Both entry points call this before anything logs, so the variable means the
/// same thing in the stdio and HTTP profiles. It used to be installed only by
/// the stdio runner, and an HTTP deployment that set it silently kept writing
/// to stderr. The fallback warning goes to stderr directly rather than through
/// the logger: the sink failed to install, so stderr is the only channel left,
/// and a `RUST_LOG` that filters warnings must not swallow the one diagnostic
/// that explains where the log *isn't* going.
pub fn install_log_file_from_env() {
    let Ok(raw) = std::env::var("MEMORY_LOG_FILE") else {
        return;
    };
    let Some(path) = resolve_log_file_path(&raw) else {
        return;
    };
    if let Err(err) = install_log_file(&path) {
        let mut event = HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::json!("main.log_file_open_failed"),
        );
        event.insert("path".to_string(), serde_json::json!(&path));
        event.insert("error".to_string(), serde_json::json!(err.to_string()));
        eprintln!(
            "{}",
            StdoutLogger::format_event_line(&event, LogLevel::Warn)
        );
    }
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

    /// The deployment's `RUST_LOG` value as a string, for a caller that must
    /// pass it somewhere that takes a level *string* rather than a logger —
    /// `MemoryService::new`, whose `log_level` argument is a `String`.
    ///
    /// Reads the same variable `from_env` does, and takes the same test
    /// override first, so a test that installs `with_level` sees its own level
    /// here too without mutating the process environment. The fallback is
    /// `"info"`: an unset dial must still report, exactly as `from_env`
    /// defaults to `info`.
    #[must_use]
    pub fn directives_from_env() -> String {
        #[cfg(test)]
        if let Some(level) = capture::override_level() {
            return level;
        }
        std::env::var(Self::LEVEL_ENV).unwrap_or_else(|_| "info".to_string())
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

    /// Build a logger from a directive list. Alias of [`Self::new`], kept for
    /// the call sites that name the input for what it is.
    fn from_directives(configured: &str) -> Self {
        Self::new(configured)
    }

    fn remove_override(&mut self, prefix: &str) {
        self.overrides.retain(|(existing, _)| existing != prefix);
    }

    /// Creates a logger from a level or a full `RUST_LOG`-style directive
    /// list.
    ///
    /// The value is the same comma-separated list `from_env` accepts: a bare
    /// level sets the default, `prefix=level` sets the level for the events
    /// whose `op` starts with that prefix at a segment boundary. Accepting the
    /// list here — rather than parsing only a bare level — is what lets a
    /// caller that already holds the deployment's `RUST_LOG` string (the stdio
    /// config, `MemoryService::build`, the HTTP runtime's service builders)
    /// hand it through unchanged; the rules used to die at this boundary.
    /// A bare level parses exactly as before.
    #[must_use]
    pub fn new(configured: &str) -> Self {
        let mut logger = Self {
            level: LogLevel::parse(first_directive(configured)),
            overrides: Vec::new(),
        };
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

        // The subscriber is the only writer now: it resolves the sink per
        // write and renders the line (see `MemoryFormat`). `install` is
        // idempotent, so a library or test that logs without having called it
        // still gets a working pipeline.
        install();
        let payload = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
        // `tracing::event!` embeds the level in the callsite, so it must be a
        // constant: one arm per level rather than a computed value.
        match level {
            LogLevel::Error => {
                tracing::event!(target: LOG_TARGET, tracing::Level::ERROR, payload = payload.as_str());
            }
            LogLevel::Warn => {
                tracing::event!(target: LOG_TARGET, tracing::Level::WARN, payload = payload.as_str());
            }
            LogLevel::Info => {
                tracing::event!(target: LOG_TARGET, tracing::Level::INFO, payload = payload.as_str());
            }
            LogLevel::Debug => {
                tracing::event!(target: LOG_TARGET, tracing::Level::DEBUG, payload = payload.as_str());
            }
            LogLevel::Trace => {
                tracing::event!(target: LOG_TARGET, tracing::Level::TRACE, payload = payload.as_str());
            }
        }
    }

    /// Formats an event into a single human-readable line.
    #[must_use]
    pub fn format_event_line(event: &HashMap<String, Value>, level: LogLevel) -> String {
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        Self::format_event_line_with_ts(event, level, &ts)
    }

    /// Formats an event with a provided timestamp — a thin wrapper over
    /// [`render_human`], kept for callers and tests that inject a clock.
    pub(crate) fn format_event_line_with_ts(
        event: &HashMap<String, Value>,
        level: LogLevel,
        ts: &str,
    ) -> String {
        render_human(event, level, ts, false)
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

/// ANSI reset, used after every coloured span.
const ANSI_RESET: &str = "\u{1b}[0m";
/// Dim, for the timestamp and field keys: present but not competing with the
/// value it names.
const ANSI_DIM: &str = "\u{1b}[2m";

/// The SGR colour for a level. Deliberately minimal — colour marks severity,
/// it does not decorate.
fn level_ansi(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "\u{1b}[31m",
        LogLevel::Warn => "\u{1b}[33m",
        LogLevel::Info => "\u{1b}[32m",
        LogLevel::Debug => "\u{1b}[34m",
        LogLevel::Trace => "\u{1b}[90m",
    }
}

/// One `key=value` token, with the key dimmed when colour is on.
fn key_token(key: &str, value: &str, ansi: bool) -> String {
    if ansi {
        format!("{ANSI_DIM}{key}{ANSI_RESET}={value}")
    } else {
        format!("{key}={value}")
    }
}

/// Whether an object is a serialized `Option`/`Result` artifact rather than a
/// real nested object, in which case it is rendered as one value instead of
/// being flattened into `key.Some=…`.
fn is_option_or_result(map: &serde_json::Map<String, Value>) -> bool {
    ["Some", "None", "Ok", "Err"]
        .iter()
        .any(|key| map.contains_key(*key))
}

/// Flatten one field into tokens. Objects become dotted keys; leaves keep the
/// existing quoting/truncation rules.
fn flatten_value(prefix: &str, value: &Value, ansi: bool, out: &mut Vec<String>) {
    match value {
        Value::Object(map) if !is_option_or_result(map) => {
            if map.is_empty() {
                out.push(key_token(prefix, "{}", ansi));
                return;
            }
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                if let Some(inner) = map.get(key) {
                    flatten_value(&format!("{prefix}.{key}"), inner, ansi, out);
                }
            }
        }
        _ => {
            let rendered = quote_if_needed(&value_to_string(value));
            out.push(key_token(prefix, &rendered, ansi));
        }
    }
}

/// Render one recorded event as a human-readable line.
///
/// Pure: the timestamp is passed in, so the exact line an operator reads is
/// assertable without a clock. Layout is `<ts> <LEVEL:>5  <tokens>` — a single
/// space separator and the level right-aligned in width five, so the columns
/// line up.
#[must_use]
pub(crate) fn render_human(
    event: &HashMap<String, Value>,
    level: LogLevel,
    ts: &str,
    ansi: bool,
) -> String {
    let level_text = format!("{:>5}", level.as_str().to_uppercase());
    let level_text = if ansi {
        format!("{}{level_text}{ANSI_RESET}", level_ansi(level))
    } else {
        level_text
    };

    let mut tokens = Vec::with_capacity(event.len());
    if let Some(op) = event.get("op") {
        tokens.push(key_token("op", &value_to_string(op), ansi));
    }
    if let Some(request_id) = event.get("request_id").and_then(Value::as_str) {
        tokens.push(key_token("req", request_id, ansi));
    }
    if let Some(ms) = event.get("duration_ms").and_then(render_duration) {
        tokens.push(key_token("duration_ms", &ms, ansi));
    }
    const SPECIAL: [&str; 3] = ["op", "request_id", "duration_ms"];
    let mut keys: Vec<&String> = event
        .keys()
        .filter(|key| !SPECIAL.contains(&key.as_str()))
        .collect();
    keys.sort();
    for key in keys {
        if let Some(value) = event.get(key) {
            flatten_value(key, value, ansi, &mut tokens);
        }
    }

    let timestamp = if ansi {
        format!("{ANSI_DIM}{ts}{ANSI_RESET}")
    } else {
        ts.to_string()
    };
    format!("{timestamp} {level_text}  {}", tokens.join("  "))
}

/// Render one recorded event as a single NDJSON object.
///
/// Pure, like [`render_human`]. Metadata (`timestamp`, `level`) is folded in
/// beside the payload so a collector parses one line and has the whole event.
#[must_use]
pub(crate) fn render_json(event: &HashMap<String, Value>, level: LogLevel, ts: &str) -> String {
    let mut object = serde_json::Map::with_capacity(event.len() + 2);
    object.insert("timestamp".to_string(), Value::String(ts.to_string()));
    object.insert(
        "level".to_string(),
        Value::String(level.as_str().to_string()),
    );
    for (key, value) in event {
        object.insert(key.clone(), value.clone());
    }
    Value::Object(object).to_string()
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
    ///
    /// The whole region is then held under [`SERIAL`]: `override_level` reads
    /// the *top* of that stack, so two tests installing concurrently each read
    /// the other's directive. A test asserting "quiet at error" found its own
    /// events emitted because a sibling's `http=debug` sat on top and reset the
    /// default to `info`. One lock makes the override unambiguous for the
    /// duration.
    pub async fn with_level<F, Fut, T>(level: &str, body: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _serialized = SERIAL.lock().await;
        let _guard = LevelOverride::install(level);
        body().await
    }

    /// One region at a time for every `with_level` caller. See `with_level`.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

        assert!(
            line.starts_with("2026-01-01T00:00:00.000+00:00  INFO  "),
            "{line}"
        );
        assert!(!line.contains("req="), "no request id, no token: {line}");
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
        assert!(line.contains("args.query=ARR"));
        assert!(line.contains("args.scope=org"));
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
        assert!(line.contains(" INFO "), "{line}");
        assert!(!line.contains('['), "no brackets: {line}");
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

        assert!(
            line.starts_with("2026-04-12T20:03:59.608616+00:00  INFO  "),
            "{line}"
        );
        assert!(line.contains("req=req_0042"));
        assert!(line.contains("op=extract.done"));
        assert!(line.contains("duration_ms=152"));
        assert!(line.contains("entities=3"));
        // request_id must not appear again in the key-value section
        let after_op = line.split("op=extract.done").nth(1).unwrap_or("");
        assert!(!after_op.contains("request_id="));
    }

    #[test]
    fn format_without_request_id_omits_the_token() {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("main.startup"));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-04-12T20:03:59.608616+00:00",
        );

        assert!(!line.contains("req="), "no placeholder token: {line}");
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

    /// `StdoutLogger::new` must accept the same directive list the rest of the
    /// deployment writes. It used to parse only a bare level: every caller that
    /// passed a raw `RUST_LOG` value (the stdio config, `MemoryService::build`)
    /// silently lost every `prefix=level` rule, so the documented subsystem
    /// dial did nothing for service-internal events.
    #[test]
    fn new_parses_a_directive_list_not_just_a_bare_level() {
        let logger = StdoutLogger::new("info,embedding.backfill_started=warn");

        assert!(
            !logger.is_event_enabled(LogLevel::Info, "embedding.backfill_started"),
            "the named op must be held back at its directive level"
        );
        assert!(
            logger.is_event_enabled(LogLevel::Info, "embedding.backfill_progress"),
            "an op no directive names keeps the default level"
        );
        assert!(
            logger.is_event_enabled(LogLevel::Warn, "embedding.backfill_started"),
            "the directive raises the op to warn, it does not silence it"
        );
    }

    /// A blank or whitespace-only `MEMORY_LOG_FILE` means "unset", not "write
    /// to a file called nothing" — the sink must not be installed for it, and
    /// a real path must survive trimming.
    #[test]
    fn memory_log_file_path_is_normalized() {
        assert_eq!(resolve_log_file_path(""), None);
        assert_eq!(resolve_log_file_path("   "), None);
        assert_eq!(
            resolve_log_file_path("  /tmp/log.txt  "),
            Some("/tmp/log.txt".to_string())
        );
    }

    /// A bare level behaves byte-for-byte as before — this is the whole
    /// existing call surface (`new("error")`, `new("info")`) and it must not
    /// move an inch.
    #[test]
    fn new_with_a_bare_level_parses_exactly_as_before() {
        let error_logger = StdoutLogger::new("error");
        assert!(!error_logger.is_enabled(LogLevel::Warn));
        assert!(error_logger.is_enabled(LogLevel::Error));
        assert!(error_logger.is_event_enabled(LogLevel::Error, "any.op"));

        let info_logger = StdoutLogger::new("info");
        assert!(info_logger.is_enabled(LogLevel::Info));
        assert!(!info_logger.is_enabled(LogLevel::Debug));
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

    /// Colour is for a human at a terminal and nothing else. `clig.dev`:
    /// disable it when the destination is not a TTY, when `NO_COLOR` is set,
    /// or when the terminal is dumb — and never write escape codes into a file.
    ///
    /// `always` is the explicit override an operator uses when a wrapper hides
    /// the TTY check, but it still cannot colour a file sink or defeat
    /// `NO_COLOR`.
    #[test]
    fn ansi_is_on_only_for_a_colour_terminal() {
        use ColorMode::{Always, Auto, Never};

        // auto: on only for a real, colour-capable terminal.
        assert!(use_ansi(false, true, false, "xterm-256color", Auto));
        assert!(
            !use_ansi(false, false, false, "xterm-256color", Auto),
            "not a TTY"
        );
        assert!(
            !use_ansi(false, true, true, "xterm-256color", Auto),
            "NO_COLOR"
        );
        assert!(!use_ansi(false, true, false, "dumb", Auto), "TERM=dumb");
        assert!(
            !use_ansi(true, true, false, "xterm-256color", Auto),
            "file sink"
        );

        // always: overrides the TTY and TERM checks, but not NO_COLOR or a sink.
        assert!(use_ansi(false, false, false, "dumb", Always));
        assert!(
            !use_ansi(false, false, true, "dumb", Always),
            "NO_COLOR wins"
        );
        assert!(
            !use_ansi(true, true, false, "xterm-256color", Always),
            "sink wins"
        );

        // never: off everywhere.
        assert!(!use_ansi(false, true, false, "xterm-256color", Never));
    }

    fn event(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    /// The line an operator reads: unbracketed microsecond timestamp, a
    /// right-aligned level, then `k=v` tokens with the operation first and the
    /// correlation id next. Nested objects are flattened to dotted keys so a
    /// payload does not hide behind `args={…}`.
    #[test]
    fn render_human_flattens_and_orders_fields() {
        let event = event(&[
            ("op", json!("ingest.done")),
            ("request_id", json!("req_0042")),
            ("duration_ms", json!(152u64)),
            ("args", json!({"source_id": "note-1"})),
            ("result", json!({"episode_id": "episode:x"})),
        ]);

        let line = render_human(&event, LogLevel::Info, "2026-10-06T12:34:56.789012Z", false);

        assert_eq!(
            line,
            "2026-10-06T12:34:56.789012Z  INFO  op=ingest.done  req=req_0042  \
             duration_ms=152  args.source_id=note-1  result.episode_id=episode:x"
        );
    }

    /// The reference's spacing: one separator, level right-aligned in width
    /// five, so a five-character level gets a single leading space and a
    /// four-character one gets two. Alignment is what lets a column of levels
    /// be scanned.
    #[test]
    fn render_human_aligns_every_level() {
        for (level, rendered) in [
            (LogLevel::Info, " INFO"),
            (LogLevel::Warn, " WARN"),
            (LogLevel::Debug, "DEBUG"),
            (LogLevel::Error, "ERROR"),
            (LogLevel::Trace, "TRACE"),
        ] {
            let line = render_human(
                &event(&[("op", json!("x"))]),
                level,
                "2026-10-06T12:34:56.789012Z",
                false,
            );
            assert!(
                line.contains(&format!("Z {rendered} ")),
                "{level:?} must be right-aligned in width five: {line}"
            );
        }
    }

    /// `req=-` was noise on every line that had no request: the field is simply
    /// absent when there is no correlation id.
    #[test]
    fn render_human_omits_an_absent_request_id() {
        let line = render_human(
            &event(&[("op", json!("main.startup"))]),
            LogLevel::Info,
            "2026-10-06T12:34:56.789012Z",
            false,
        );
        assert!(!line.contains("req="), "no request id, no token: {line}");
    }

    /// Values keep the existing quoting/truncation/artifact rules, so nested
    /// flattening does not change how a leaf is rendered.
    #[test]
    fn render_human_reuses_value_rendering() {
        let event = event(&[
            ("op", json!("resolve.done")),
            ("name", json!("Dmitry Ivanov")),
            ("list", json!(["a", "b", "c"])),
            ("args", json!({"nested": {"deep": true}})),
        ]);

        let line = render_human(&event, LogLevel::Info, "2026-10-06T12:34:56.789012Z", false);

        assert!(line.contains("name=\"Dmitry Ivanov\""), "{line}");
        assert!(line.contains("list=[a,b,c]"), "{line}");
        assert!(line.contains("args.nested.deep=true"), "{line}");
    }

    /// No ANSI when the caller says not to; escape codes only when told.
    #[test]
    fn render_human_only_colours_when_asked() {
        let event = event(&[("op", json!("x"))]);
        let plain = render_human(
            &event,
            LogLevel::Error,
            "2026-10-06T12:34:56.789012Z",
            false,
        );
        let coloured = render_human(&event, LogLevel::Error, "2026-10-06T12:34:56.789012Z", true);

        assert!(!plain.contains('\u{1b}'), "{plain}");
        assert!(coloured.contains('\u{1b}'), "{coloured}");
        assert!(coloured.contains("ERROR"), "{coloured}");
    }

    /// JSON mode is one object per line, machine-parseable, with the metadata
    /// folded in beside the payload so a collector needs no second source.
    /// Asserted by parsing, not string-comparison: key order is not a promise.
    #[test]
    fn render_json_is_one_object_per_event() {
        let event = event(&[
            ("op", json!("ingest.done")),
            ("request_id", json!("req_0042")),
            ("duration_ms", json!(152u64)),
            ("args", json!({"source_id": "note-1"})),
        ]);

        let line = render_json(&event, LogLevel::Info, "2026-10-06T12:34:56.789012Z");

        assert!(!line.contains('\n'), "one object per line: {line}");
        let parsed: Value = serde_json::from_str(&line).expect("valid NDJSON object");
        assert_eq!(parsed["timestamp"], "2026-10-06T12:34:56.789012Z");
        assert_eq!(parsed["level"], "info");
        assert_eq!(parsed["op"], "ingest.done");
        assert_eq!(parsed["request_id"], "req_0042");
        assert_eq!(parsed["duration_ms"], 152);
        assert_eq!(parsed["args"]["source_id"], "note-1");
    }

    /// `MEMORY_LOG_FORMAT=json` selects the machine format; anything else,
    /// including a typo, stays human-readable.
    #[test]
    fn log_format_parse_selects_json() {
        assert_eq!(LogFormat::parse("json"), LogFormat::Json);
        assert_eq!(LogFormat::parse("  JSON "), LogFormat::Json);
        assert_eq!(LogFormat::parse("text"), LogFormat::Text);
        assert_eq!(LogFormat::parse(""), LogFormat::Text);
        assert_eq!(LogFormat::parse("yaml"), LogFormat::Text);
    }

    /// A buffer the formatter writes into, so a test can read the emitted line
    /// without touching stderr or a file.
    #[derive(Clone, Default)]
    struct SharedBuf(std::sync::Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().expect("buffer").clone()).expect("utf8")
        }
    }

    struct SharedBufGuard(std::sync::Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBufGuard {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("buffer").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for SharedBuf {
        type Writer = SharedBufGuard;

        fn make_writer(&'a self) -> Self::Writer {
            SharedBufGuard(self.0.clone())
        }
    }

    fn subscriber_with(buffer: SharedBuf, format: LogFormat) -> impl Subscriber + Send + Sync {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(MemoryFormat { format })
                    .with_writer(buffer)
                    .with_ansi(false),
            )
            .with(
                Targets::new()
                    .with_target(LOG_TARGET, tracing::Level::TRACE)
                    .with_default(FOREIGN_DEFAULT_LEVEL),
            )
    }

    /// Our events carry the serialised map in `payload`; the formatter flattens
    /// it through the same renderer the pure tests assert on.
    #[test]
    fn format_event_renders_our_payload() {
        let buffer = SharedBuf::default();
        let subscriber = subscriber_with(buffer.clone(), LogFormat::Text);
        tracing::subscriber::with_default(subscriber, || {
            tracing::event!(
                target: LOG_TARGET,
                tracing::Level::INFO,
                payload = r#"{"op":"ingest.done","request_id":"req_1","args":{"source_id":"note-1"}}"#
            );
        });

        let line = buffer.contents();
        assert!(line.contains("op=ingest.done"), "{line}");
        assert!(line.contains("req=req_1"), "{line}");
        assert!(line.contains("args.source_id=note-1"), "{line}");
    }

    /// `MEMORY_LOG_FORMAT=json` yields one parseable object per event.
    #[test]
    fn format_event_renders_the_json_mode() {
        let buffer = SharedBuf::default();
        let subscriber = subscriber_with(buffer.clone(), LogFormat::Json);
        tracing::subscriber::with_default(subscriber, || {
            tracing::event!(
                target: LOG_TARGET,
                tracing::Level::INFO,
                payload = r#"{"op":"x"}"#
            );
        });

        let parsed: Value = serde_json::from_str(buffer.contents().trim()).expect("json");
        assert_eq!(parsed["op"], "x");
        assert_eq!(parsed["level"], "info");
    }

    /// A third-party event (no `payload` field) is rendered from its own
    /// fields, not dropped to nothing.
    #[test]
    fn format_event_renders_a_foreign_event() {
        let buffer = SharedBuf::default();
        let subscriber = subscriber_with(buffer.clone(), LogFormat::Text);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                target: "surrealdb::kvs",
                count = 8,
                message = "level-0 slowdown"
            );
        });

        let line = buffer.contents();
        assert!(line.contains("surrealdb::kvs:"), "{line}");
        assert!(line.contains("level-0 slowdown"), "{line}");
        assert!(line.contains("count=8"), "{line}");
    }

    /// The `Targets` policy keeps third-party noise below its default out while
    /// letting its warnings through.
    #[test]
    fn foreign_events_below_the_policy_are_dropped() {
        let buffer = SharedBuf::default();
        let subscriber = subscriber_with(buffer.clone(), LogFormat::Text);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "noisy_dep", "should not appear");
            tracing::error!(target: "noisy_dep", "should appear");
        });

        let line = buffer.contents();
        assert!(!line.contains("should not appear"), "{line}");
        assert!(line.contains("should appear"), "{line}");
    }

    /// The test capture observes the same line the formatter emits — the seam
    /// the rest of the suite asserts through.
    #[test]
    fn format_event_records_for_capture() {
        let guard = capture::install();
        let buffer = SharedBuf::default();
        let subscriber = subscriber_with(buffer, LogFormat::Text);
        tracing::subscriber::with_default(subscriber, || {
            tracing::event!(
                target: LOG_TARGET,
                tracing::Level::INFO,
                payload = r#"{"op":"capture.probe"}"#
            );
        });

        assert!(
            guard
                .lines()
                .iter()
                .any(|line| line.contains("op=capture.probe")),
            "capture must see the emitted line"
        );
    }
}
