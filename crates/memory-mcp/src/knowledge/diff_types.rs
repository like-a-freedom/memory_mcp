//! Request and view types for the knowledge-owned diff command.
//!
//! These describe a bi-temporal comparison over knowledge's facts, so
//! they sit with the command that builds them rather than in the app
//! type bag. They are plain data: no behaviour, no dependency on how
//! the answer is produced.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
