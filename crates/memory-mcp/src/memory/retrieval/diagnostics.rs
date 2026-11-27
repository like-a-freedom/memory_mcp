//! Feature-gated, explicitly requested evaluation receipts. Never production logs.
use std::sync::{Arc, Mutex};
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

/// Payload shape at an actual completed candidate boundary; no source text or keys.
#[derive(Debug, Clone, Serialize)]
pub struct CandidateInventory {
    pub stage: String,
    pub elapsed_ms: f64,
    pub records: usize,
    pub content_bytes: usize,
    pub max_content_bytes: usize,
    pub index_keys: usize,
    pub index_key_bytes: usize,
    pub max_index_key_bytes: usize,
    pub unique_contents: usize,
    pub unique_sources: usize,
    /// Exact identities observed in this fetched pool for the requested source.
    /// An empty list describes this boundary only, never an unobserved stage.
    pub tracked_source_fact_ids: Vec<String>,
}

/// Actual dictionary construction counts in one completed lexical operation.
#[derive(Debug, Clone, Serialize)]
pub struct LexicalWork {
    pub stage: &'static str,
    pub elapsed_ms: f64,
    pub records: usize,
    pub content_dictionaries: usize,
    pub index_dictionaries: usize,
}

/// Explicit evaluation sink. Implementations may durably flush each boundary.
pub trait CandidateObserver: Send + Sync {
    fn observe(&self, inventory: &CandidateInventory);
    fn observe_work(&self, _work: &LexicalWork) {}
    fn observe_phase(&self, _stage: &str, _elapsed_ms: f64) {}
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
                candidates: Vec::new(),
                observer: None,
            }),
        }
    }

    pub fn with_candidate_observer(
        source_episode: &str,
        observer: Arc<dyn CandidateObserver>,
    ) -> Self {
        let mut capture = Self::new(source_episode);
        if let Ok(recorder) = capture.recorder.get_mut() {
            recorder.observer = Some(observer);
        }
        capture
    }

    pub fn candidates(&self) -> Result<Vec<CandidateInventory>, crate::error::MemoryError> {
        self.recorder
            .lock()
            .map(|recorder| recorder.candidates.clone())
            .map_err(|_| {
                crate::error::MemoryError::Validation(
                    "evaluation evidence recorder poisoned".into(),
                )
            })
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
    candidates: Vec<CandidateInventory>,
    observer: Option<Arc<dyn CandidateObserver>>,
}

pub(super) type EvidenceRecorder<'a> = Option<&'a Mutex<Recorder>>;

pub(super) fn record_ranked(
    trace: EvidenceRecorder<'_>,
    stage: &str,
    facts: &[super::types::RankedContextFact],
    cutoff: chrono::DateTime<chrono::Utc>,
) {
    let source = trace.and_then(|trace| trace.lock().ok().map(|r| r.source_episode.clone()));
    let Some(source) = source else { return };
    let mut seen_sources = std::collections::HashSet::new();
    let source_rank = facts
        .iter()
        .take_while(|fact| fact.fact.source_episode != source)
        .filter(|fact| seen_sources.insert(fact.fact.source_episode.as_str()))
        .count();
    let items = facts
        .iter()
        .enumerate()
        .filter(|(_, fact)| fact.fact.source_episode == source)
        .map(|(rank, fact)| {
            let mut item = super::scoring::ranked_fact_to_item(
                fact.clone(),
                cutoff,
                super::fact_decayed_confidence,
            );
            item.provenance["eval_candidate_position"] = serde_json::json!({
                "candidate_rank": rank,
                "first_source_rank": source_rank,
                "pool_size": facts.len(),
            });
            item
        })
        .collect::<Vec<_>>();
    record(trace, stage, &items);
}

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

pub(super) fn record_candidates(
    trace: EvidenceRecorder<'_>,
    stage: &str,
    records: &[serde_json::Value],
) {
    let Some(trace) = trace else { return };
    use super::filtering::{raw_array, raw_object};
    use crate::storage::value_helpers::json_string;
    use std::collections::HashSet;
    let mut contents = HashSet::new();
    let mut sources = HashSet::new();
    let tracked_source = trace
        .lock()
        .ok()
        .map(|recorder| recorder.source_episode.clone());
    let mut inventory = CandidateInventory {
        stage: stage.to_owned(),
        elapsed_ms: 0.0,
        records: records.len(),
        content_bytes: 0,
        max_content_bytes: 0,
        index_keys: 0,
        index_key_bytes: 0,
        max_index_key_bytes: 0,
        unique_contents: 0,
        unique_sources: 0,
        tracked_source_fact_ids: Vec::new(),
    };
    for record in records {
        let Some(map) = raw_object(record) else {
            continue;
        };
        if let Some(content) = map.get("content").and_then(json_string) {
            inventory.content_bytes += content.len();
            inventory.max_content_bytes = inventory.max_content_bytes.max(content.len());
            contents.insert(content);
        }
        if let Some(source) = map
            .get("source_episode")
            .or_else(|| map.get("episode_id"))
            .and_then(json_string)
        {
            sources.insert(source);
            if tracked_source.as_deref() == Some(source)
                && let Some(id) = map
                    .get("fact_id")
                    .and_then(crate::storage::value_helpers::unwrap_record_string)
            {
                inventory.tracked_source_fact_ids.push(id);
            }
        }
        for key in map
            .get("index_keys")
            .and_then(raw_array)
            .into_iter()
            .flatten()
            .filter_map(json_string)
        {
            inventory.index_keys += 1;
            inventory.index_key_bytes += key.len();
            inventory.max_index_key_bytes = inventory.max_index_key_bytes.max(key.len());
        }
    }
    inventory.unique_contents = contents.len();
    inventory.unique_sources = sources.len();
    let observer = if let Ok(mut recorder) = trace.lock() {
        inventory.elapsed_ms = recorder.started.elapsed().as_secs_f64() * 1000.0;
        recorder.candidates.push(inventory.clone());
        recorder.observer.clone()
    } else {
        None
    };
    // The callback runs after releasing the recorder lock and before expensive
    // normalization. An explicit caller can flush completed metadata even if
    // synchronous work later outlives a cooperative deadline.
    if let Some(observer) = observer {
        observer.observe(&inventory);
    }
}

pub(super) fn record_work(trace: EvidenceRecorder<'_>, mut work: LexicalWork) {
    let Some(trace) = trace else { return };
    let observer = trace.lock().ok().and_then(|recorder| {
        work.elapsed_ms = recorder.started.elapsed().as_secs_f64() * 1000.0;
        recorder.observer.clone()
    });
    if let Some(observer) = observer {
        observer.observe_work(&work);
    }
}

/// Flush only a phase boundary name and read-relative clock; never source data.
pub(super) fn record_phase(trace: EvidenceRecorder<'_>, stage: &str) {
    if let Some(trace) = trace {
        let observation = trace.lock().ok().map(|recorder| {
            (
                recorder.observer.clone(),
                recorder.started.elapsed().as_secs_f64() * 1000.0,
            )
        });
        if let Some((Some(observer), elapsed_ms)) = observation {
            observer.observe_phase(stage, elapsed_ms);
        }
    }
}
