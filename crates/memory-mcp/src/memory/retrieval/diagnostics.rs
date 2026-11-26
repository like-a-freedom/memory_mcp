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

/// One explicitly requested diagnostic read. Owned by the caller so completed
/// stages survive cancellation or a storage error in the read future.
pub struct ContextEvidenceCapture {
    pub(super) recorder: Mutex<Recorder>,
}

impl ContextEvidenceCapture {
    pub fn new(source_episode: &str) -> Self {
        Self {
            recorder: Mutex::new(Recorder {
                started: Instant::now(),
                read_started: false,
                source_episode: source_episode.to_owned(),
                stages: Vec::new(),
            }),
        }
    }

    pub(super) fn begin(&self) -> Result<String, crate::error::MemoryError> {
        let mut recorder = self.recorder.lock().map_err(|_| {
            crate::error::MemoryError::Validation("evaluation evidence recorder poisoned".into())
        })?;
        if recorder.read_started {
            return Err(crate::error::MemoryError::Validation(
                "evaluation evidence capture supports only one read".into(),
            ));
        }
        recorder.read_started = true;
        recorder.started = Instant::now();
        Ok(recorder.source_episode.clone())
    }

    /// Only observed boundaries are present. A partial capture is not a
    /// completed context, and an empty stage is not proof of absent candidates.
    pub fn stages(&self) -> Result<Vec<RetrievalStage>, crate::error::MemoryError> {
        self.recorder
            .lock()
            .map(|recorder| recorder.stages.clone())
            .map_err(|_| {
                crate::error::MemoryError::Validation(
                    "evaluation evidence recorder poisoned".into(),
                )
            })
    }
}

pub(super) struct Recorder {
    pub(super) started: Instant,
    read_started: bool,
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
