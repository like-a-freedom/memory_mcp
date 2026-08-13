use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrepareIngestionReviewRequest {
    pub source_text: Option<String>,
    pub draft_episode_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngestionReviewSource {
    pub source_text: Option<String>,
    pub draft_episode_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngestionReviewSummary {
    pub total: usize,
    pub pending: usize,
    pub approved: usize,
    pub rejected: usize,
    pub edited: usize,
    pub committable: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestionReviewItem {
    pub item_id: String,
    pub status: String,
    pub kind: String,
    pub fact_type: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    pub entity_links: Vec<String>,
    pub confidence: f64,
    pub t_valid: DateTime<Utc>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestionReviewBundle {
    pub source: IngestionReviewSource,
    pub items: Vec<IngestionReviewItem>,
    pub summary: IngestionReviewSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommitIngestionReviewRequest {
    pub items: Vec<IngestionReviewItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitIngestionReviewOutcome {
    pub committed_count: usize,
    pub fact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffRequest {
    pub target_type: String,
    pub target_id: Option<String>,
    pub as_of_left: DateTime<Utc>,
    pub as_of_right: DateTime<Utc>,
    pub time_axis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffTarget {
    pub target_type: String,
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffViewRange {
    pub as_of_left: DateTime<Utc>,
    pub as_of_right: DateTime<Utc>,
    pub time_axis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffChange {
    pub fact_id: String,
    pub change_type: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    pub t_valid: DateTime<Utc>,
    pub t_ingested: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffSummary {
    pub left_count: usize,
    pub right_count: usize,
    pub added_count: usize,
    pub removed_count: usize,
    pub unchanged_count: usize,
    pub change_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffView {
    pub target: DiffTarget,
    pub range: DiffViewRange,
    pub summary: DiffSummary,
    pub changes: Vec<DiffChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleDashboard {
    pub active_facts: usize,
    pub archival_candidates: usize,
    pub archival_candidate_ids: Vec<String>,
    pub communities: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LifecycleDefaults {
    pub archival_age_days: u32,
    pub decay_threshold: f64,
    pub decay_half_life_days: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LifecycleView {
    pub dashboard: LifecycleDashboard,
    pub defaults: LifecycleDefaults,
    pub recent_actions: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveCandidatesOutcome {
    pub dry_run: bool,
    pub target_ids: Vec<String>,
    pub archived_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreArchivedOutcome {
    pub target_ids: Vec<String>,
    pub restored_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecomputeDecayOutcome {
    pub dry_run: bool,
    pub invalidated: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RebuildCommunitiesOutcome {
    pub dry_run: bool,
    pub rebuilt: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleCommand {
    ArchiveCandidates {
        target_ids: Vec<String>,
        dry_run: bool,
        confirmed: bool,
    },
    RestoreArchived {
        target_ids: Vec<String>,
        confirmed: bool,
    },
    RecomputeDecay {
        dry_run: bool,
        confirmed: bool,
    },
    RebuildCommunities {
        dry_run: bool,
        confirmed: bool,
    },
}

#[derive(Debug)]
pub enum LifecycleCommandOutcome {
    ArchiveCandidates(ArchiveCandidatesOutcome),
    RestoreArchived(RestoreArchivedOutcome),
    RecomputeDecay(RecomputeDecayOutcome),
    RebuildCommunities(RebuildCommunitiesOutcome),
}

impl IngestionReviewSummary {
    #[must_use]
    pub fn from_items(items: &[IngestionReviewItem]) -> Self {
        let mut pending = 0;
        let mut approved = 0;
        let mut rejected = 0;
        let mut edited = 0;

        for item in items {
            match item.status.as_str() {
                "approved" => approved += 1,
                "rejected" => rejected += 1,
                "edited" => edited += 1,
                _ => pending += 1,
            }
        }

        Self {
            total: items.len(),
            pending,
            approved,
            rejected,
            edited,
            committable: approved + edited,
        }
    }
}
