use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct IngestArgs {
    #[arg(long)]
    pub source_type: String,
    #[arg(long)]
    pub source_id: String,
    #[arg(long)]
    pub content: String,
    #[arg(long)]
    pub t_ref: String,
    #[arg(long, default_value = "org")]
    pub scope: String,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub t_ingested: Option<String>,
    #[arg(long)]
    pub visibility_scope: Option<String>,
    #[arg(long = "policy-tag")]
    pub policy_tags: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ExtractArgs {
    #[arg(long)]
    pub episode_id: Option<String>,
    #[arg(long)]
    pub content: Option<String>,
    #[arg(long)]
    pub text: Option<String>,
    #[arg(long)]
    pub source_type: Option<String>,
    #[arg(long)]
    pub source_id: Option<String>,
    #[arg(long)]
    pub t_ref: Option<String>,
    #[arg(long)]
    pub scope: Option<String>,
    #[arg(long = "zero-shot-label")]
    pub zero_shot_labels: Option<Vec<String>>,
}

#[derive(Debug, Args)]
pub struct ResolveArgs {
    #[arg(long)]
    pub entity_type: String,
    #[arg(long)]
    pub canonical_name: String,
    #[arg(long)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Args)]
pub struct InvalidateArgs {
    #[arg(long)]
    pub fact_id: String,
    #[arg(long)]
    pub reason: String,
    #[arg(long)]
    pub t_invalid: String,
}

#[derive(Debug, Args)]
pub struct ExplainArgs {
    /// JSON array string of context items
    #[arg(long)]
    pub context_items: String,
}

#[derive(Debug, Args)]
pub struct AssembleContextArgs {
    #[arg(long)]
    pub query: String,
    #[arg(long, default_value = "org")]
    pub scope: String,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long = "fact-type")]
    pub fact_types: Vec<String>,
    #[arg(long, default_value = "")]
    pub as_of: String,
    #[arg(long, default_value_t = 5)]
    pub budget: i32,
    #[arg(long = "view-mode")]
    pub view_mode: Option<String>,
    #[arg(long)]
    pub window_start: Option<String>,
    #[arg(long)]
    pub window_end: Option<String>,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    pub dir: PathBuf,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long, default_value = "team")]
    pub scope: String,
    #[arg(long, default_value_t = 2)]
    pub interval_secs: u64,
}
