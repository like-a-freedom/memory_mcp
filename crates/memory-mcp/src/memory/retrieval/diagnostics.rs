//! Feature-gated, explicitly requested evaluation receipts. Never production logs.
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;

use crate::models::AssembledContextItem;

#[derive(Debug, Clone, Serialize)]
pub struct RetrievalStage {
    pub stage: String,
    /// Offset from the start of this read; not an isolated stage duration.
    pub elapsed_ms: f64,
    pub items: Vec<AssembledContextItem>,
}

#[derive(Debug, Serialize)]
pub struct ContextEvidence {
    pub stages: Vec<RetrievalStage>,
    pub items: Vec<AssembledContextItem>,
}

pub(super) struct Recorder {
    pub(super) started: Instant,
    pub(super) source_episode: String,
    pub(super) stages: Vec<RetrievalStage>,
}

pub(super) type EvidenceRecorder<'a> = Option<&'a Mutex<Recorder>>;

pub(super) fn record(trace: EvidenceRecorder<'_>, stage: &str, items: &[AssembledContextItem]) {
    if let Some(trace) = trace
        && let Ok(mut recorder) = trace.lock()
    {
        let items = items
            .iter()
            .filter(|item| item.source_episode == recorder.source_episode)
            .cloned()
            .collect();
        let elapsed_ms = recorder.started.elapsed().as_secs_f64() * 1000.0;
        recorder.stages.push(RetrievalStage {
            stage: stage.to_owned(),
            elapsed_ms,
            items,
        });
    }
}
