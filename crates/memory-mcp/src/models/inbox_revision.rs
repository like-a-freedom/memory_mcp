//! Typed vocabulary for durable inbox revisions.
//!
//! Each logical filesystem document has a lineage derived from its normalized
//! path relative to the inbox; each distinct set of raw bytes is an immutable
//! revision identified by SHA-256. A revision becomes claimable only after its
//! normalized prepared content is durably persisted.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Revision identity: `inbox_revision:<64 lowercase hex chars>` over the raw
/// byte hash. Record-safe because the body is fixed-width hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InboxRevisionId(pub String);

impl InboxRevisionId {
    /// Builds a revision ID from the deterministic raw-byte hash.
    pub fn from_hash(hash: &str) -> Self {
        Self(format!("inbox_revision:{hash}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InboxRevisionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Durable revision states. Exactly these four; `processing_stage` is recovery
/// metadata, not a fifth public state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxRevisionState {
    Discovered,
    Processing,
    Processed,
    Failed,
}

impl InboxRevisionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Processing => "processing",
            Self::Processed => "processed",
            Self::Failed => "failed",
        }
    }
}

/// Recovery metadata describing where inside `ingest → extract` a crash
/// happened. Not a public state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxProcessingStage {
    Prepared,
    Ingesting,
    Extracting,
    Complete,
}

impl InboxProcessingStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Ingesting => "ingesting",
            Self::Extracting => "extracting",
            Self::Complete => "complete",
        }
    }
}

/// Bounded failure classification for one revision cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxFailureClass {
    Validation,
    Corrupt,
    Io,
    Storage,
    Model,
    Timeout,
    Channel,
    OtherTransient,
}

impl InboxFailureClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::Corrupt => "corrupt",
            Self::Io => "io",
            Self::Storage => "storage",
            Self::Model => "model",
            Self::Timeout => "timeout",
            Self::Channel => "channel",
            Self::OtherTransient => "other_transient",
        }
    }
}

/// One durable inbox revision row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxRevisionRecord {
    pub revision_id: InboxRevisionId,
    pub lineage: String,
    pub relative_path: String,
    pub content_sha256: String,
    pub source_type: String,
    pub t_ref: DateTime<Utc>,
    /// Durable normalized content produced from the immutable raw-byte revision.
    pub prepared_content: Option<String>,
    pub state: InboxRevisionState,
    pub processing_stage: InboxProcessingStage,
    /// Deterministically computed before ingest; closes the ingest-return crash window.
    pub expected_episode_id: String,
    pub episode_id: Option<String>,
    pub attempt_count: u32,
    pub failure_count: u32,
    pub retry_generation: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub failure_class: Option<InboxFailureClass>,
    pub last_error: Option<String>,
    pub discovered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
}

/// A revision leased to one processor with its durable prepared-content snapshot.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ClaimedInboxRevision {
    pub record: InboxRevisionRecord,
    pub prepared_content: String,
    pub lease: InboxRevisionLease,
}

/// Lease identity bound to one processor task.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct InboxRevisionLease {
    pub revision_id: InboxRevisionId,
    pub owner: String,
}
