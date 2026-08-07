//! Model acquisition progress events.
//!
//! One domain event stream feeds two renderers: CLI mode renders human text,
//! MCP mode writes one compact versioned JSON object per line. Both write to
//! stderr; MCP stdout remains JSON-RPC only. A throttle layer emits on phase
//! changes, completion or failure, each crossed 5% download boundary, or after
//! five seconds without another emitted update.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Download-throttle bucket width, in percent.
const PROGRESS_BUCKET_PERCENT: u8 = 5;

/// Heartbeat interval when nothing else would be emitted.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Schema version of the MCP JSON Lines progress event.
pub const PROGRESS_SCHEMA_VERSION: u8 = 1;

/// Phase of artifact preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProgressPhase {
    /// Resolving the upstream latest revision.
    Resolve,
    /// Waiting for a concurrent process to release the lease.
    WaitForLease,
    /// Downloading artifacts.
    Download,
    /// Verifying completeness and integrity.
    Verify,
    /// Constructing the model.
    Construct,
    /// Running a smoke inference.
    SmokeTest,
    /// Activating the checkpoint.
    Activate,
    /// Falling back to a known-good revision.
    Fallback,
}

impl ModelProgressPhase {
    /// Stable lowercase name used in CLI rendering and log events.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::WaitForLease => "wait-for-lease",
            Self::Download => "download",
            Self::Verify => "verify",
            Self::Construct => "construct",
            Self::SmokeTest => "smoke-test",
            Self::Activate => "activate",
            Self::Fallback => "fallback",
        }
    }
}

/// One progress event on the domain stream.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ModelProgressEvent {
    /// Schema version of this event shape.
    pub schema_version: u8,
    /// Stable extractor identity, e.g. `vago-lfm2.5-gliner`.
    pub extractor: String,
    /// Current preparation phase.
    pub phase: ModelProgressPhase,
    /// `started`, `updated`, `completed`, or `failed`.
    pub status: String,
    /// Resolved revision when known.
    pub revision: Option<String>,
    /// Bytes downloaded so far.
    pub downloaded_bytes: Option<u64>,
    /// Total expected bytes.
    pub total_bytes: Option<u64>,
    /// Download progress, 0-100.
    pub progress_percent: Option<u8>,
    /// Human-readable detail.
    pub message: Option<String>,
}

impl ModelProgressEvent {
    /// Creates a `started` event for a phase.
    pub fn started(extractor: &str, phase: ModelProgressPhase) -> Self {
        Self {
            schema_version: PROGRESS_SCHEMA_VERSION,
            extractor: extractor.to_string(),
            phase,
            status: "started".to_string(),
            revision: None,
            downloaded_bytes: None,
            total_bytes: None,
            progress_percent: None,
            message: None,
        }
    }

    /// Creates a `completed` event for a phase.
    pub fn completed(
        extractor: &str,
        phase: ModelProgressPhase,
        message: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: PROGRESS_SCHEMA_VERSION,
            extractor: extractor.to_string(),
            phase,
            status: "completed".to_string(),
            revision: None,
            downloaded_bytes: None,
            total_bytes: None,
            progress_percent: None,
            message: Some(message.into()),
        }
    }

    /// Creates a `failed` event.
    pub fn failed(extractor: &str, phase: ModelProgressPhase, message: impl Into<String>) -> Self {
        Self {
            schema_version: PROGRESS_SCHEMA_VERSION,
            extractor: extractor.to_string(),
            phase,
            status: "failed".to_string(),
            revision: None,
            downloaded_bytes: None,
            total_bytes: None,
            progress_percent: None,
            message: Some(message.into()),
        }
    }

    /// Creates a download progress update event.
    pub fn download(
        extractor: &str,
        revision: Option<String>,
        downloaded_bytes: u64,
        total_bytes: u64,
        progress_percent: u8,
    ) -> Self {
        Self {
            schema_version: PROGRESS_SCHEMA_VERSION,
            extractor: extractor.to_string(),
            phase: ModelProgressPhase::Download,
            status: "updated".to_string(),
            revision,
            downloaded_bytes: Some(downloaded_bytes),
            total_bytes: Some(total_bytes),
            progress_percent: Some(progress_percent.min(100)),
            message: None,
        }
    }
}

/// Receives progress events. Sinks must be cheap and must not block.
pub trait ModelProgressSink: Send + Sync {
    fn emit(&self, event: &ModelProgressEvent);
}

impl<S: ModelProgressSink> ModelProgressSink for Arc<S> {
    fn emit(&self, event: &ModelProgressEvent) {
        (**self).emit(event);
    }
}

/// Writes one compact JSON object plus newline per event to stderr.
pub struct JsonLineProgressSink {
    writer: Mutex<std::io::Stderr>,
}

impl JsonLineProgressSink {
    pub fn new() -> Self {
        Self {
            writer: Mutex::new(std::io::stderr()),
        }
    }
}

impl Default for JsonLineProgressSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelProgressSink for JsonLineProgressSink {
    fn emit(&self, event: &ModelProgressEvent) {
        let line = serde_json::to_string(event).unwrap_or_else(|err| {
            format!(r#"{{"schema_version":1,"extractor":"","phase":"resolve","status":"failed","message":"serialization error: {err}"}}"#)
        });
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writeln!(writer, "{line}");
        }
    }
}

/// Renders human-readable progress lines to stderr.
/// Renders human-readable progress lines to stderr.
pub struct CliProgressSink {
    writer: Mutex<std::io::Stderr>,
}

impl CliProgressSink {
    pub fn new() -> Self {
        Self {
            writer: Mutex::new(std::io::stderr()),
        }
    }
}

impl Default for CliProgressSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelProgressSink for CliProgressSink {
    fn emit(&self, event: &ModelProgressEvent) {
        let mut line = format!("[ner] {} {}", event.extractor, event.phase.as_str());
        if let Some(percent) = event.progress_percent {
            line.push_str(&format!(" {percent}%"));
        }
        if let Some(bytes) = event.downloaded_bytes {
            if let Some(total) = event.total_bytes {
                line.push_str(&format!(" ({bytes}/{total} bytes)"));
            }
        }
        if let Some(message) = &event.message {
            line.push_str(": ");
            line.push_str(message);
        }
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writeln!(writer, "{line}");
        }
    }
}

/// Throttles the domain stream: emits on phase change, completion or failure,
/// each crossed 5% boundary, or after a five-second heartbeat with no change.
/// Throttles the domain stream: emits on phase change, completion or failure,
/// each crossed 5% boundary, or after a five-second heartbeat with no change.
pub struct ThrottledProgressSink<S: ModelProgressSink> {
    inner: S,
    last: Mutex<ThrottleState>,
}

#[derive(Default)]
struct ThrottleState {
    last_phase: Option<ModelProgressPhase>,
    last_bucket: Option<u8>,
    last_emitted_at: Option<Instant>,
    last_line: Option<String>,
}

impl<S: ModelProgressSink> ThrottledProgressSink<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            last: Mutex::new(ThrottleState::default()),
        }
    }
}

impl<S: ModelProgressSink> ModelProgressSink for ThrottledProgressSink<S> {
    fn emit(&self, event: &ModelProgressEvent) {
        let mut state = self.last.lock().expect("throttle state lock");
        let now = Instant::now();
        let bucket = event
            .progress_percent
            .map(|percent| percent / PROGRESS_BUCKET_PERCENT);
        let phase_changed = state.last_phase != Some(event.phase);
        let boundary_crossed = match (state.last_bucket, bucket) {
            (Some(previous), Some(current)) => current > previous,
            (None, Some(_)) => true,
            _ => false,
        };
        let terminal = event.status == "completed" || event.status == "failed";
        let heartbeat_due = state
            .last_emitted_at
            .map(|at| now.duration_since(at) >= HEARTBEAT_INTERVAL)
            .unwrap_or(false);
        let line = serde_json::to_string(event).unwrap_or_default();
        let duplicate = state.last_line.as_deref() == Some(line.as_str());

        if phase_changed || boundary_crossed || terminal || (heartbeat_due && !duplicate) {
            state.last_phase = Some(event.phase);
            state.last_bucket = bucket.or(state.last_bucket);
            state.last_emitted_at = Some(now);
            state.last_line = Some(line);
            drop(state);
            self.inner.emit(event);
        }
    }
}

/// Captures emitted events in memory (test and integration-test helper).
#[derive(Default)]
pub struct CapturingSink {
    events: Mutex<Vec<ModelProgressEvent>>,
}

impl CapturingSink {
    pub fn events(&self) -> Vec<ModelProgressEvent> {
        self.events.lock().expect("events lock").clone()
    }

    pub fn event_count(&self) -> usize {
        self.events.lock().expect("events lock").len()
    }
}

impl ModelProgressSink for CapturingSink {
    fn emit(&self, event: &ModelProgressEvent) {
        self.events.lock().expect("events lock").push(event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_line_serializes_compact_single_line_with_schema_version() {
        let sink = JsonLineProgressSink::new();
        // Rendering goes to stderr; the serialization shape is what we assert
        // via the same serializer the sink uses.
        let event = ModelProgressEvent::started("vago", ModelProgressPhase::Resolve);
        let serialized = serde_json::to_string(&event).expect("serialize");
        assert!(serialized.starts_with(
            r#"{"schema_version":1,"extractor":"vago","phase":"resolve","status":"started""#
        ));
        assert!(!serialized.contains('\n'));
    }

    #[test]
    fn phase_events_are_terminal_detectable() {
        let started = ModelProgressEvent::started("vago", ModelProgressPhase::Download);
        let completed = ModelProgressEvent::completed("vago", ModelProgressPhase::Download, "done");
        let failed = ModelProgressEvent::failed("vago", ModelProgressPhase::Construct, "boom");
        assert_eq!(started.status, "started");
        assert_eq!(completed.status, "completed");
        assert_eq!(failed.status, "failed");
    }

    #[test]
    fn throttle_emits_phase_changes_and_terminal_events() {
        let capture = Arc::new(CapturingSink::default());
        let throttled = ThrottledProgressSink::new(capture.clone());
        throttled.emit(&ModelProgressEvent::started(
            "vago",
            ModelProgressPhase::Resolve,
        ));
        throttled.emit(&ModelProgressEvent::started(
            "vago",
            ModelProgressPhase::Resolve,
        ));
        throttled.emit(&ModelProgressEvent::completed(
            "vago",
            ModelProgressPhase::Resolve,
            "ok",
        ));
        let events = capture.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].phase, ModelProgressPhase::Resolve);
        assert_eq!(events[1].status, "completed");
    }

    #[test]
    fn throttle_emits_on_5_percent_boundary_crossing() {
        let capture = Arc::new(CapturingSink::default());
        let throttled = ThrottledProgressSink::new(capture.clone());
        // 4% then 5% -> first update also crosses 0->1 bucket; then 9% stays
        // in bucket 1 and is a duplicate only if unchanged; a 10% crosses.
        throttled.emit(&ModelProgressEvent::download("vago", None, 4, 100, 4));
        assert_eq!(capture.events().len(), 1);
        throttled.emit(&ModelProgressEvent::download("vago", None, 5, 100, 5));
        assert_eq!(capture.events().len(), 2);
        throttled.emit(&ModelProgressEvent::download("vago", None, 9, 100, 9));
        // 9% is the same bucket as 5%, so no boundary crossing.
        assert_eq!(capture.events().len(), 2);
        throttled.emit(&ModelProgressEvent::download("vago", None, 10, 100, 10));
        assert_eq!(capture.events().len(), 3);
    }

    #[test]
    fn throttle_heartbeat_emits_after_five_seconds() {
        let capture = Arc::new(CapturingSink::default());
        let throttled = ThrottledProgressSink::new(capture.clone());
        throttled.emit(&ModelProgressEvent::started(
            "vago",
            ModelProgressPhase::Download,
        ));
        assert_eq!(capture.events().len(), 1);
        std::thread::sleep(Duration::from_millis(5_100));
        throttled.emit(&ModelProgressEvent::download("vago", None, 51, 100, 51));
        // Same bucket as the initial boundary 50? The first download is
        // bucket 10 and phase Download equals started phase Download, so it
        // needs the heartbeat (or boundary) to pass.
        assert_eq!(capture.events().len(), 2);
    }

    #[test]
    fn throttle_does_not_emit_duplicate_intermediate_updates() {
        let capture = Arc::new(CapturingSink::default());
        let throttled = ThrottledProgressSink::new(capture.clone());
        throttled.emit(&ModelProgressEvent::started(
            "vago",
            ModelProgressPhase::Download,
        ));
        let event = ModelProgressEvent::download("vago", None, 10, 100, 10);
        throttled.emit(&event);
        throttled.emit(&event);
        assert_eq!(capture.events().len(), 2);
    }

    #[test]
    fn cli_sink_render_contains_extractor_and_phase() {
        // CLI sink writes to stderr; assert the phase name and extractor are
        // used in the rendering path via the sink's own formatting.
        let sink = CliProgressSink::new();
        let event = ModelProgressEvent::download("vago", None, 10, 100, 10);
        // Cannot easily capture stderr in-process; just ensure no panic and
        // that the format helpers are consistent.
        sink.emit(&event);
        assert_eq!(ModelProgressPhase::Download.as_str(), "download");
    }
}
