use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct IngestArgs {
    /// Source type identifier — one of "email", "chat", "doc", "note", "code", "web", "other"
    #[arg(long)]
    pub source_type: String,
    /// Unique identifier within the source type (e.g. message-id, file path, URL)
    #[arg(long)]
    pub source_id: String,
    /// Raw content to store as the episode body
    #[arg(long)]
    pub content: String,
    /// Reference (valid) time in ISO 8601 — e.g. "2026-06-30T10:00:00Z"
    #[arg(long)]
    pub t_ref: String,
    /// Access scope — "org", "team", "personal", or "private-domain:<domain>"
    #[arg(long, default_value = "org")]
    pub scope: String,
    /// Project or namespace tag for grouping episodes
    #[arg(long)]
    pub project: Option<String>,
    /// Override ingestion timestamp in ISO 8601 (defaults to now)
    #[arg(long)]
    pub t_ingested: Option<String>,
    /// Visibility scope override for entity resolution visibility
    #[arg(long)]
    pub visibility_scope: Option<String>,
    /// Policy tags for content governance (repeatable: --policy-tag tag1 --policy-tag tag2)
    #[arg(long = "policy-tag")]
    pub policy_tags: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ExtractArgs {
    /// Episode ID from a prior ingest call (mutually exclusive with --content/--text)
    #[arg(long)]
    pub episode_id: Option<String>,
    /// Raw content to extract from inline (mutually exclusive with --episode-id)
    #[arg(long)]
    pub content: Option<String>,
    /// Plain text alternative to --content (same semantics, different alias)
    #[arg(long)]
    pub text: Option<String>,
    /// Source type when using inline content (required if --content/--text used)
    #[arg(long)]
    pub source_type: Option<String>,
    /// Source ID when using inline content (required if --content/--text used)
    #[arg(long)]
    pub source_id: Option<String>,
    /// Reference time in ISO 8601 for inline content (required if --content/--text used)
    #[arg(long)]
    pub t_ref: Option<String>,
    /// Access scope for inline content extraction
    #[arg(long)]
    pub scope: Option<String>,
    /// Zero-shot classification labels for entity type detection (repeatable)
    #[arg(long = "zero-shot-label")]
    pub zero_shot_labels: Option<Vec<String>>,
}

#[derive(Debug, Args)]
pub struct ResolveArgs {
    /// Entity type — e.g. "person", "organization", "project", "concept"
    #[arg(long)]
    pub entity_type: String,
    /// Canonical (preferred) name that aliases should resolve to
    #[arg(long)]
    pub canonical_name: String,
    /// Alias names to merge under the canonical entity (repeatable: --aliases Alias1 --aliases Alias2)
    #[arg(long)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Args)]
pub struct InvalidateArgs {
    /// Fact ID to invalidate (from extract or assemble-context output)
    #[arg(long)]
    pub fact_id: String,
    /// Human-readable reason for invalidation (stored in audit trail)
    #[arg(long)]
    pub reason: String,
    /// Invalidation time in ISO 8601 — e.g. "2026-06-30T00:00:00Z"
    #[arg(long)]
    pub t_invalid: String,
}

#[derive(Debug, Args)]
pub struct ExplainArgs {
    /// JSON array of context items as returned by `assemble-context`
    /// Example: '[{"fact_id":"fact:abc","score":0.85}]'
    #[arg(long)]
    pub context_items: String,
}

#[derive(Debug, Args)]
pub struct AssembleContextArgs {
    /// Natural language query — facts will be ranked by relevance to this text
    #[arg(long)]
    pub query: String,
    /// Access scope filter — "org", "team", "personal", or "private-domain:<domain>"
    #[arg(long, default_value = "org")]
    pub scope: String,
    /// Project filter to narrow results to a specific namespace
    #[arg(long)]
    pub project: Option<String>,
    /// Filter by fact type (repeatable: --fact-type commitment --fact-type preference)
    #[arg(long = "fact-type")]
    pub fact_types: Vec<String>,
    /// Point-in-time query in ISO 8601 — only facts valid at this time (default: now)
    #[arg(long, default_value = "")]
    pub as_of: String,
    /// Maximum number of context items to return (default: 5)
    #[arg(long, default_value_t = 5)]
    pub budget: i32,
    /// View mode — "current" (default), "all", or "diff"
    #[arg(long = "view-mode")]
    pub view_mode: Option<String>,
    /// Temporal window start in ISO 8601
    #[arg(long)]
    pub window_start: Option<String>,
    /// Temporal window end in ISO 8601
    #[arg(long)]
    pub window_end: Option<String>,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// Directory to watch for new files
    pub dir: PathBuf,
    /// Project namespace for auto-ingested files
    #[arg(long)]
    pub project: Option<String>,
    /// Access scope for auto-ingested files (default: team)
    #[arg(long, default_value = "team")]
    pub scope: String,
    /// Polling interval in seconds (default: 2)
    #[arg(long, default_value_t = 2)]
    pub interval_secs: u64,
}

/// Internal lifecycle-capture args — consumed by hook scripts, not a public tool.
///
/// Hidden from `--help` via `#[command(hide = true)]` on the subcommand variant.
/// See ADR-0016 AD-4 and `docs/agent_integration/CONTRACT.md`.
#[derive(Debug, Args)]
pub struct LifecycleCaptureArgs {
    /// JSON-encoded `NormalizedHostEvent` (event_kind, task_fingerprint, scope, etc.)
    #[arg(long)]
    pub event: String,
    /// JSON-encoded `InvocationContext` (origin, session_id, etc.)
    #[arg(long)]
    pub context: String,
}

/// Internal lifecycle-recall args — consumed by hook scripts, not a public tool.
///
/// Hidden from `--help` via `#[command(hide = true)]` on the subcommand variant.
/// See ADR-0016 AD-5 and `docs/agent_integration/CONTRACT.md`.
#[derive(Debug, Args)]
pub struct LifecycleRecallArgs {
    /// JSON-encoded `NormalizedHostEvent` (event_kind, task_fingerprint, scope, etc.)
    #[arg(long)]
    pub event: String,
    /// JSON-encoded `InvocationContext` (origin, session_id, etc.)
    #[arg(long)]
    pub context: String,
}
