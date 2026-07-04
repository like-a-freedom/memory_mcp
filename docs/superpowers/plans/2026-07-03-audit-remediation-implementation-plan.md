# Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove dangling and non-wired functionality from `memory_mcp`, then bring the public app surface and supporting domain logic into strict DRY, YAGNI, KISS, and DDD alignment without regressing the six canonical memory tools.

**Architecture:** Execute this as one coordinated remediation because the findings are coupled through the same `MemoryMcp -> MemoryService -> DbClient` seams. First introduce explicit domain primitives for scope and lifecycle policy, then complete the currently fake app workflows in the service layer, thin the MCP adapter, centralize temporal visibility rules, and finally stabilize the critical tests and remove dead compatibility code.

**Tech Stack:** Rust edition 2024, tokio, rmcp, surrealdb 3.1, serde/serde_json, chrono, schemars, thiserror, tempfile

## Global Constraints

- Edition `2024`
- `main.rs` stays thin; business logic lives in `src/service/`
- MCP tool/app schemas remain flat `snake_case`; no nested `payload` wrappers
- Feature flags are additive and `default = []`
- Facts remain immutable; use invalidation, never deletion
- Scope discipline must honor `personal` / `team` / `org` / `private-domain`
- Unknown scopes must fail explicitly; no silent namespace fallback in public flows
- Tool responses remain decision-ready and keep `status`, `guidance`, `has_more`, `total_count` contracts where applicable
- Production code uses `MemoryError` and `?`; no `unwrap()` or `expect()` outside tests
- Use Octocode for code navigation (`graphrag`, `semantic_search`, `view_signatures`, `structural_search`); do not use `grep`, `rg`, or `find`
- Quality gate before handoff: `cargo check --all-targets`, `cargo check --all-targets --all-features`, `cargo clippy --all-targets`, `cargo fmt --all --check`, `cargo test`

---

## Scope Check

This plan intentionally stays single. The app wiring bugs, scope bugs, lifecycle policy drift, temporal-query drift, and failing ignored suites are not independent subsystems; they all share the same public entry points, storage queries, and test harness.

## File Structure

### Files created by this plan

- `src/service/scope.rs` — typed scope parser and namespace resolver used by handlers and service flows
- `src/service/apps/types.rs` — shared value objects for ingestion review, diff, lifecycle, and graph app flows
- `src/service/apps/ingestion_review.rs` — real ingestion-review preparation and commit use cases
- `src/service/apps/diff.rs` — real temporal diff use case
- `src/service/apps/lifecycle.rs` — service-layer lifecycle dashboard and archive/restore/recompute actions
- `src/mcp/handlers/apps.rs` — app-specific MCP adapter methods extracted from the god-file
- `tests/apps_ingestion_review.rs` — integration coverage for review preparation and commit persistence
- `tests/apps_diff.rs` — integration coverage for temporal diff behavior

### Files modified heavily by this plan

- `Cargo.toml` — add additive `mcp-apps` feature flag
- `src/service.rs` — register new app and scope modules
- `src/service/core.rs` — delegate app workflows to focused service modules instead of direct DB writes in handlers
- `src/service/core/helpers.rs` — remove silent fallback namespace logic
- `src/service/lifecycle/archival.rs` — use centralized temporal visibility and service-layer archive invariants
- `src/service/lifecycle/decay.rs` — use centralized lifecycle policy
- `src/storage/queries.rs` — centralize fact visibility predicate and fix archival query drift
- `src/storage/client.rs` — expose only query helpers that use the shared temporal predicate
- `src/config/constants.rs` — keep lifecycle defaults in one place
- `src/config/lifecycle.rs` — expose a single policy source for decay and archival defaults
- `src/mcp/handlers.rs` — shrink to router plus canonical tools; move app-specific code out
- `src/mcp/session.rs` — enforce TTL instead of storing decorative expiry metadata
- `src/mcp/resources.rs` — feature-gate optional app resources
- `src/models.rs` — remove duplicate default-scope helper and add any small shared domain types that truly belong here
- `src/mcp/parsers.rs` — remove duplicate default-scope helper if still present after Task 1
- `tests/common/mod.rs` — deterministic service/db fixtures with unique temp storage
- `tests/lifecycle_archival.rs` — make the currently ignored archival assertions runnable
- `tests/lifecycle_decay.rs` — make the currently ignored decay assertions runnable
- `tests/explain_provenance.rs` — make the currently ignored provenance assertions runnable
- `README.md` — document optional `mcp-apps` exposure and verification commands

## Task 1: Typed Scope and Unified Lifecycle Policy

**Files:**
- Create: `src/service/scope.rs`
- Modify: `src/service.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service/core/helpers.rs`
- Modify: `src/config/constants.rs`
- Modify: `src/config/lifecycle.rs`
- Modify: `src/models.rs`
- Modify: `src/mcp/parsers.rs`
- Test: `src/service/scope.rs`

**Interfaces:**
- Consumes: `LifecycleConfig`, `MemoryError`, current `namespace_for_scope(&self, scope: &str) -> String`
- Produces: `pub(crate) enum MemoryScope`, `pub(crate) fn parse(raw: &str) -> Result<MemoryScope, MemoryError>`, `pub(crate) fn namespace(&self, namespaces: &[String]) -> Result<String, MemoryError>`, `pub(crate) struct LifecyclePolicy { archival_age_days: u32, decay_confidence_threshold: f64, decay_half_life_days: f64 }`

- [ ] **Step 1: Write the failing tests**

```rust
// In src/service/scope.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_scope_accepts_team_and_private_domain() {
        assert_eq!(MemoryScope::parse("team").unwrap().as_str(), "team");
        assert_eq!(
            MemoryScope::parse("private-domain").unwrap().as_str(),
            "private-domain"
        );
    }

    #[test]
    fn parse_scope_rejects_unknown_scope() {
        let err = MemoryScope::parse("org-typo").unwrap_err();
        assert!(matches!(err, MemoryError::Validation(_)));
    }

    #[test]
    fn lifecycle_policy_matches_config_defaults() {
        let policy = LifecyclePolicy::default();
        assert_eq!(policy.archival_age_days, 90);
        assert_eq!(policy.decay_confidence_threshold, 0.3);
        assert_eq!(policy.decay_half_life_days, 365.0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test parse_scope_rejects_unknown_scope --lib -- --exact`
Expected: FAIL with `use of undeclared type 'MemoryScope'` or `file not found for module 'scope'`

- [ ] **Step 3: Write the minimal implementation**

```rust
// In src/service/scope.rs
use crate::config::LifecycleConfig;
use crate::service::MemoryError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryScope {
    Personal,
    Team,
    Org,
    PrivateDomain,
}

impl MemoryScope {
    pub(crate) fn parse(raw: &str) -> Result<Self, MemoryError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "personal" => Ok(Self::Personal),
            "team" => Ok(Self::Team),
            "org" => Ok(Self::Org),
            "private-domain" | "private_domain" => Ok(Self::PrivateDomain),
            other => Err(MemoryError::Validation(format!("unknown scope: {other}"))),
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Team => "team",
            Self::Org => "org",
            Self::PrivateDomain => "private-domain",
        }
    }

    pub(crate) fn namespace(&self, namespaces: &[String]) -> Result<String, MemoryError> {
        let candidates = match self {
            Self::Personal => &["personal"][..],
            Self::Team => &["team", "org"][..],
            Self::Org => &["org"][..],
            Self::PrivateDomain => &["private-domain", "private"][..],
        };

        candidates
            .iter()
            .find_map(|candidate| namespaces.iter().find(|ns| ns.as_str() == *candidate))
            .cloned()
            .ok_or_else(|| {
                MemoryError::Validation(format!(
                    "no namespace configured for scope {}",
                    self.as_str()
                ))
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LifecyclePolicy {
    pub(crate) archival_age_days: u32,
    pub(crate) decay_confidence_threshold: f64,
    pub(crate) decay_half_life_days: f64,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            archival_age_days: 90,
            decay_confidence_threshold: 0.3,
            decay_half_life_days: 365.0,
        }
    }
}

impl From<&LifecycleConfig> for LifecyclePolicy {
    fn from(config: &LifecycleConfig) -> Self {
        Self {
            archival_age_days: config.archival_age_days,
            decay_confidence_threshold: config.decay_confidence_threshold,
            decay_half_life_days: config.decay_half_life_days,
        }
    }
}
```

```rust
// In src/service.rs
mod scope;
pub(crate) use scope::{LifecyclePolicy, MemoryScope};
```

```rust
// In src/service/core.rs
pub(crate) fn lifecycle_policy(&self) -> LifecyclePolicy {
    LifecyclePolicy::from(&self.lifecycle_config)
}
```

Delete both duplicate `default_scope()` helpers so `org` defaulting lives in one place only.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test parse_scope_accepts_team_and_private_domain --lib -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/service.rs src/service/scope.rs src/service/core.rs src/service/core/helpers.rs src/config/constants.rs src/config/lifecycle.rs src/models.rs src/mcp/parsers.rs
git commit -m "feat: add typed scope and unify lifecycle policy"
```

### Task 2: Real Service-Layer App Workflows

**Files:**
- Create: `src/service/apps/types.rs`
- Create: `src/service/apps/ingestion_review.rs`
- Create: `src/service/apps/diff.rs`
- Create: `src/service/apps/lifecycle.rs`
- Modify: `src/service/apps.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service/lifecycle/archival.rs`
- Modify: `src/service/lifecycle/decay.rs`
- Test: `tests/apps_ingestion_review.rs`
- Test: `tests/apps_diff.rs`

**Interfaces:**
- Consumes: `MemoryScope`, `LifecyclePolicy`, `MemoryService::add_fact`, `MemoryService::find_episode_record`, `MemoryService::find_fact_record`
- Produces: `pub async fn MemoryService::prepare_ingestion_review(&self, request: PrepareIngestionReviewRequest) -> Result<IngestionReviewBundle, MemoryError>`, `pub async fn MemoryService::commit_ingestion_review(&self, request: CommitIngestionReviewRequest) -> Result<CommitIngestionReviewOutcome, MemoryError>`, `pub async fn MemoryService::build_diff(&self, request: DiffRequest) -> Result<DiffView, MemoryError>`, plus the shared request/response value objects in `src/service/apps/types.rs`

- [ ] **Step 1: Write the failing tests**

```rust
// In tests/apps_ingestion_review.rs
#[tokio::test]
async fn prepare_ingestion_review_uses_episode_backed_drafts() {
    let (service, _db_client) = common::make_service_with_client().await;
    let episode_id = common::ingest_episode(&service, "draft-episode", "Alice promised a launch on Friday.").await;

    let bundle = service
        .prepare_ingestion_review(PrepareIngestionReviewRequest {
            scope: "org".to_string(),
            source_text: None,
            draft_episode_id: Some(episode_id.clone()),
        })
        .await
        .unwrap();

    assert_eq!(bundle.items.len(), 1);
    assert_eq!(bundle.items[0].source_episode.as_deref(), Some(episode_id.as_str()));
}

#[tokio::test]
async fn commit_ingestion_review_persists_approved_items() {
    let (service, db_client) = common::make_service_with_client().await;
    let episode_id = common::ingest_episode(&service, "commit-episode", "OpenAI approved the rollout.").await;
    let bundle = service
        .prepare_ingestion_review(PrepareIngestionReviewRequest {
            scope: "org".to_string(),
            source_text: None,
            draft_episode_id: Some(episode_id.clone()),
        })
        .await
        .unwrap();

    let outcome = service
        .commit_ingestion_review(CommitIngestionReviewRequest {
            scope: "org".to_string(),
            items: bundle.items,
            approved_item_ids: vec!["draft:fact:0".to_string()],
        })
        .await
        .unwrap();

    let facts = db_client.select_table("fact", "org").await.unwrap();
    assert_eq!(outcome.committed_count, 1);
    assert!(facts.iter().any(|row| row.get("source_episode").and_then(|v| v.as_str()) == Some(episode_id.as_str())));
}
```

```rust
// In tests/apps_diff.rs
#[tokio::test]
async fn build_diff_reports_temporal_changes() {
    let (service, _db_client) = common::make_service_with_client().await;
    let left = chrono::DateTime::parse_from_rfc3339("2026-03-01T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
    let right = chrono::DateTime::parse_from_rfc3339("2026-03-10T00:00:00Z").unwrap().with_timezone(&chrono::Utc);

    common::seed_fact_at(&service, "org", "Budget is 10", left).await;
    common::seed_fact_at(&service, "org", "Budget is 15", right).await;

    let diff = service
        .build_diff(DiffRequest {
            scope: "org".to_string(),
            target_type: "scope".to_string(),
            target_id: None,
            as_of_left: left,
            as_of_right: right,
            time_axis: DiffTimeAxis::Valid,
        })
        .await
        .unwrap();

    assert_eq!(diff.summary.change_count, 1);
    assert_eq!(diff.changes[0].change_kind, "added");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test commit_ingestion_review_persists_approved_items --test apps_ingestion_review -- --exact`
Expected: FAIL with `no method named 'prepare_ingestion_review' found for struct 'MemoryService'`

- [ ] **Step 3: Write the minimal implementation**

```rust
// In src/service/apps/types.rs
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct ReviewItem {
    pub item_id: String,
    pub status: String,
    pub kind: String,
    pub content: String,
    pub quote: String,
    pub source_episode: Option<String>,
    pub fact_type: String,
    pub confidence: f64,
    pub entity_links: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct IngestionReviewBundle {
    pub items: Vec<ReviewItem>,
}

#[derive(Debug, Clone)]
pub struct PrepareIngestionReviewRequest {
    pub scope: String,
    pub source_text: Option<String>,
    pub draft_episode_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommitIngestionReviewRequest {
    pub scope: String,
    pub items: Vec<ReviewItem>,
    pub approved_item_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CommitIngestionReviewOutcome {
    pub committed_fact_ids: Vec<String>,
    pub committed_count: usize,
}

#[derive(Debug, Clone)]
pub enum DiffTimeAxis {
    Valid,
    Transaction,
}

#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub change_count: usize,
}

#[derive(Debug, Clone)]
pub struct DiffChange {
    pub change_kind: String,
    pub fact_id: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct DiffView {
    pub summary: DiffSummary,
    pub changes: Vec<DiffChange>,
}

impl DiffView {
    pub fn from_snapshots(
        before: Vec<serde_json::Value>,
        after: Vec<serde_json::Value>,
        _request: DiffRequest,
    ) -> Self {
        let before_ids = before
            .iter()
            .filter_map(|row| row.get("fact_id").and_then(|value| value.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        let mut changes = Vec::new();

        for row in after {
            let fact_id = row
                .get("fact_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            if !before_ids.contains(fact_id.as_str()) {
                changes.push(DiffChange {
                    change_kind: "added".to_string(),
                    fact_id,
                    content: row
                        .get("content")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
            }
        }

        Self {
            summary: DiffSummary {
                change_count: changes.len(),
            },
            changes,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiffRequest {
    pub scope: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub as_of_left: DateTime<Utc>,
    pub as_of_right: DateTime<Utc>,
    pub time_axis: DiffTimeAxis,
}
```

```rust
// In src/service/apps/ingestion_review.rs
pub async fn prepare_ingestion_review(
    service: &MemoryService,
    request: PrepareIngestionReviewRequest,
) -> Result<IngestionReviewBundle, MemoryError> {
    let scope = MemoryScope::parse(&request.scope)?;
    let _namespace = scope.namespace(&service.namespaces)?;
    let episode_id = request
        .draft_episode_id
        .clone()
        .ok_or_else(|| MemoryError::Validation("draft_episode_id is required".into()))?;
    let (episode, _) = service.find_episode_record(&episode_id).await?;
    let content = episode
        .as_ref()
        .and_then(|row| row.get("content"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| MemoryError::Validation("draft episode content is missing".into()))?;

    Ok(IngestionReviewBundle {
        items: vec![ReviewItem {
            item_id: "draft:fact:0".to_string(),
            status: "pending".to_string(),
            kind: "draft_fact".to_string(),
            content: content.to_string(),
            quote: content.to_string(),
            source_episode: Some(episode_id),
            fact_type: "note".to_string(),
            confidence: 0.9,
            entity_links: Vec::new(),
        }],
    })
}

pub async fn commit_ingestion_review(
    service: &MemoryService,
    request: CommitIngestionReviewRequest,
) -> Result<CommitIngestionReviewOutcome, MemoryError> {
    let scope = MemoryScope::parse(&request.scope)?;
    let namespace = scope.namespace(&service.namespaces)?;
    let mut committed_fact_ids = Vec::new();

    for item in request.items.into_iter().filter(|item| {
        request
            .approved_item_ids
            .iter()
            .any(|approved| approved == &item.item_id)
    }) {
        let source_episode = item
            .source_episode
            .clone()
            .ok_or_else(|| MemoryError::Validation("review item is missing source_episode".into()))?;
        let fact_id = service
            .add_fact(
                &item.fact_type,
                &item.content,
                &item.quote,
                &source_episode,
                chrono::Utc::now(),
                &namespace,
                item.confidence,
                item.entity_links.clone(),
                Vec::new(),
                crate::models::Provenance::agent_observation(&source_episode),
            )
            .await?;
        committed_fact_ids.push(fact_id);
    }

    Ok(CommitIngestionReviewOutcome {
        committed_count: committed_fact_ids.len(),
        committed_fact_ids,
    })
}
```

```rust
// In src/service/apps/diff.rs
pub async fn build_diff(
    service: &MemoryService,
    request: DiffRequest,
) -> Result<DiffView, MemoryError> {
    let scope = MemoryScope::parse(&request.scope)?;
    let namespace = scope.namespace(&service.namespaces)?;
    let left = crate::service::normalize_dt(request.as_of_left);
    let right = crate::service::normalize_dt(request.as_of_right);

    let before = service.db_client.select_facts_filtered(&namespace, &left, None, 200).await?;
    let after = service.db_client.select_facts_filtered(&namespace, &right, None, 200).await?;

    DiffView::from_snapshots(before, after, request)
}
```

```rust
// In src/service.rs
pub use apps::types::{
    CommitIngestionReviewOutcome, CommitIngestionReviewRequest, DiffRequest, DiffTimeAxis,
    DiffView, IngestionReviewBundle, PrepareIngestionReviewRequest, ReviewItem,
};
```

Wire these service functions through `src/service/core.rs` as thin delegators instead of using `db_client` directly from handlers.

```rust
// In src/service/core.rs
pub async fn prepare_ingestion_review(
    &self,
    request: PrepareIngestionReviewRequest,
) -> Result<IngestionReviewBundle, MemoryError> {
    crate::service::apps::ingestion_review::prepare_ingestion_review(self, request).await
}

pub async fn commit_ingestion_review(
    &self,
    request: CommitIngestionReviewRequest,
) -> Result<CommitIngestionReviewOutcome, MemoryError> {
    crate::service::apps::ingestion_review::commit_ingestion_review(self, request).await
}

pub async fn build_diff(&self, request: DiffRequest) -> Result<DiffView, MemoryError> {
    crate::service::apps::diff::build_diff(self, request).await
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test build_diff_reports_temporal_changes --test apps_diff -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/service.rs src/service/core.rs src/service/apps.rs src/service/apps/types.rs src/service/apps/ingestion_review.rs src/service/apps/diff.rs src/service/apps/lifecycle.rs src/service/lifecycle/archival.rs src/service/lifecycle/decay.rs tests/apps_ingestion_review.rs tests/apps_diff.rs
git commit -m "feat: move app workflows into service layer"
```

### Task 3: Thin MCP App Adapter, Feature Gating, and Session TTL

**Files:**
- Modify: `Cargo.toml`
- Create: `src/mcp/handlers/apps.rs`
- Modify: `src/mcp/handlers.rs`
- Modify: `src/mcp/session.rs`
- Modify: `src/mcp/resources.rs`
- Modify: `src/mcp/params.rs`
- Test: `src/mcp/session.rs`
- Test: `src/mcp/handlers.rs`

**Interfaces:**
- Consumes: `prepare_ingestion_review`, `commit_ingestion_review`, `build_diff`, `archive_candidates`, `restore_archived`
- Produces: `SessionManager::get_valid(&self, session_id: &str) -> Result<AppSessionState, ErrorData>`, `SessionManager::purge_expired(&self) -> usize`, additive Cargo feature `mcp-apps`

- [ ] **Step 1: Write the failing tests**

```rust
// In src/mcp/session.rs
#[tokio::test]
async fn get_valid_rejects_expired_session() {
    let manager = SessionManager::new();
    manager
        .insert(
            "ses:9999".to_string(),
            AppSessionState {
                app: "diff".to_string(),
                scope: "org".to_string(),
                expires_at: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
                payload: serde_json::json!({"app": "diff"}),
            },
        )
        .await;

    let err = manager.get_valid("ses:9999").await.unwrap_err();
    assert!(err.to_string().contains("expired"));
}
```

```rust
// In src/mcp/handlers.rs tests
#[tokio::test]
async fn app_command_commit_review_persists_and_closes_session() {
    let mcp = create_test_mcp().await;
    let open = mcp
        .open_app(rmcp::handler::server::tool::Parameters(OpenAppParams {
            app: "ingestion_review".to_string(),
            scope: "org".to_string(),
            target_type: None,
            target_id: None,
            from_entity_id: None,
            to_entity_id: None,
            source_text: Some("Alice approved the launch".to_string()),
            draft_episode_id: None,
            as_of: None,
            as_of_left: None,
            as_of_right: None,
            time_axis: None,
            view: None,
            cursor: None,
            page_size: None,
            max_depth: None,
            ttl_seconds: Some(30),
        }))
        .await
        .unwrap();

    let response = mcp
        .app_command(rmcp::handler::server::tool::Parameters(AppCommandParams {
            session_id: open.0.result.session_id.clone(),
            action: "commit_review".to_string(),
            item_ids: vec!["draft:fact:0".to_string()],
            target_ids: Vec::new(),
            target_id: None,
            item_id: None,
            patch_json: None,
            reason: None,
            dry_run: None,
            confirmed: Some(true),
            format: None,
            direction: None,
            depth: None,
        }))
        .await
        .unwrap();

    assert_eq!(response.0.result.details.as_ref().unwrap()["committed_count"], 1);
    assert!(mcp.read_app_resource_payload("ingestion_review", &open.0.result.session_id).await.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test get_valid_rejects_expired_session --lib -- --exact`
Expected: FAIL with `struct 'AppSessionState' has no field named 'expires_at'` or `no method named 'get_valid'`

- [ ] **Step 3: Write the minimal implementation**

```toml
# In Cargo.toml
[features]
default = []
cli-watch = ["notify"]
mcp-apps = []
```

```rust
// In src/mcp/session.rs
#[derive(Debug, Clone)]
pub(crate) struct AppSessionState {
    pub(crate) app: String,
    pub(crate) scope: String,
    pub(crate) expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) payload: Value,
}

impl SessionManager {
    pub async fn get_valid(&self, session_id: &str) -> Result<AppSessionState, ErrorData> {
        let session = self
            .sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| invalid_params(format!("Unknown or closed app session: {session_id}")))?;

        if session.expires_at.is_some_and(|expires_at| expires_at <= chrono::Utc::now()) {
            self.sessions.write().await.remove(session_id);
            return Err(invalid_params(format!("App session expired: {session_id}")));
        }

        Ok(session)
    }
}
```

```rust
// In src/mcp/handlers.rs
#[cfg(feature = "mcp-apps")]
pub async fn open_app(
    &self,
    params: Parameters<OpenAppParams>,
) -> Result<Json<ToolResponse<OpenAppResult>>, ErrorData> {
    crate::mcp::handlers::apps::open_app(self, params).await
}

#[cfg(feature = "mcp-apps")]
pub async fn app_command(
    &self,
    params: Parameters<AppCommandParams>,
) -> Result<Json<ToolResponse<AppCommandResult>>, ErrorData> {
    crate::mcp::handlers::apps::app_command(self, params).await
}
```

```rust
// In src/mcp/resources.rs
#[cfg(feature = "mcp-apps")]
const PUBLIC_APPS: [(&str, &str); 5] = [/* existing app list */];
```

Move the app-specific open/command branches into `src/mcp/handlers/apps.rs` so `src/mcp/handlers.rs` becomes the public router plus canonical tool methods.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test app_command_commit_review_persists_and_closes_session --lib --features mcp-apps -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/mcp/handlers.rs src/mcp/handlers/apps.rs src/mcp/session.rs src/mcp/resources.rs src/mcp/params.rs
git commit -m "feat: gate optional apps and enforce session ttl"
```

### Task 4: Centralize Bi-Temporal Visibility and Lifecycle Invariants

**Files:**
- Modify: `src/storage/queries.rs`
- Modify: `src/storage/client.rs`
- Modify: `src/service/lifecycle/archival.rs`
- Modify: `src/service/lifecycle/decay.rs`
- Modify: `src/service/apps/lifecycle.rs`
- Test: `src/storage/queries.rs`
- Test: `tests/lifecycle_archival.rs`
- Test: `tests/lifecycle_decay.rs`

**Interfaces:**
- Consumes: `BI_TEMPORAL_WHERE`, `LifecyclePolicy`, `run_archival_pass`, `run_decay_pass`
- Produces: `pub(crate) fn build_fact_visibility_clause(cutoff_var: &str) -> String`, archival/decay flows that read policy from one source only

- [ ] **Step 1: Write the failing tests**

```rust
// In src/storage/queries.rs
#[test]
fn build_select_active_facts_by_episode_query_uses_full_bitemporal_visibility() {
    let (sql, vars) = build_select_active_facts_by_episode_query("episode:1", "2026-05-13T00:00:00Z", 1);
    assert!(sql.contains("t_valid <= type::datetime($cutoff)"));
    assert!(sql.contains("t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)"));
    assert!(sql.contains("t_invalid_ingested > type::datetime($cutoff)"));
    assert_eq!(vars["episode_id"], "episode:1");
}
```

```rust
// In tests/lifecycle_archival.rs
#[tokio::test]
async fn archival_pass_archives_old_episode_without_visible_facts() {
    let (service, db_client) = common::make_service_with_client().await;
    let old = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
    let episode_id = common::seed_episode_backed_fact_with_source_id(&service, "org", "Legacy note", old, "archivable-episode").await;

    service
        .invalidate(memory_mcp::models::InvalidateRequest {
            fact_id: db_client.select_table("fact", "org").await.unwrap()[0]["fact_id"].as_str().unwrap().to_string(),
            reason: "superseded".to_string(),
            t_invalid: chrono::Utc::now(),
        })
        .await
        .unwrap();

    let archived = memory_mcp::service::lifecycle::archival::run_archival_pass(&service, 90)
        .await
        .unwrap();

    let episode = db_client.select_one(&episode_id, "org").await.unwrap().unwrap();
    assert_eq!(archived, 1);
    assert_eq!(episode["status"], "archived");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test build_select_active_facts_by_episode_query_uses_full_bitemporal_visibility --lib -- --exact`
Expected: FAIL because the SQL only checks `t_invalid`

- [ ] **Step 3: Write the minimal implementation**

```rust
// In src/storage/queries.rs
pub(crate) fn build_fact_visibility_clause(cutoff_var: &str) -> String {
    format!(
        "t_valid <= type::datetime({cutoff}) \
         AND (t_ingested IS NONE OR t_ingested <= type::datetime({cutoff})) \
         AND (t_invalid IS NONE OR t_invalid > type::datetime({cutoff}) OR t_invalid_ingested > type::datetime({cutoff}))",
        cutoff = cutoff_var
    )
}

pub fn build_select_active_facts_by_episode_query(
    episode_id: &str,
    cutoff: &str,
    limit: i32,
) -> (String, Value) {
    let visibility = build_fact_visibility_clause("$cutoff");
    (
        format!(
            "SELECT * FROM fact WHERE source_episode = $episode_id AND {visibility} LIMIT $limit"
        ),
        json!({"episode_id": episode_id, "cutoff": cutoff, "limit": limit}),
    )
}
```

```rust
// In src/service/lifecycle/archival.rs
pub async fn run_archival_pass(
    service: &MemoryService,
    age_days: u32,
) -> Result<usize, MemoryError> {
    let policy = service.lifecycle_policy();
    let age_days = age_days.max(policy.archival_age_days);
    // existing loop, but archive/restore decisions must only happen through
    // service-level lifecycle helpers, never direct handler DB writes
}
```

```rust
// In src/service/lifecycle/decay.rs
let policy = service.lifecycle_policy();
let threshold = policy.decay_confidence_threshold;
let half_life_days = policy.decay_half_life_days;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test archival_pass_archives_old_episode_without_visible_facts --test lifecycle_archival -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/storage/queries.rs src/storage/client.rs src/service/lifecycle/archival.rs src/service/lifecycle/decay.rs src/service/apps/lifecycle.rs tests/lifecycle_archival.rs tests/lifecycle_decay.rs
git commit -m "fix: centralize temporal visibility and lifecycle invariants"
```

### Task 5: Deterministic Critical Tests, Dead-Code Cleanup, and Docs

**Files:**
- Modify: `tests/common/mod.rs`
- Modify: `tests/explain_provenance.rs`
- Modify: `tests/lifecycle_archival.rs`
- Modify: `tests/lifecycle_decay.rs`
- Modify: `src/service/triple_extractor.rs`
- Modify: `src/cli/runtime.rs`
- Modify: `src/models.rs`
- Modify: `README.md`
- Test: `tests/explain_provenance.rs`

**Interfaces:**
- Consumes: current env-backed test setup, `MemoryService::new_from_env`, `NoOpTripleExtractor`, deprecated CLI compatibility re-exports
- Produces: unique-temp test fixture helpers, runnable non-ignored critical suites, no dead compatibility types kept without a clear caller

- [ ] **Step 1: Write the failing tests**

```rust
// In tests/explain_provenance.rs
#[tokio::test]
async fn explain_populates_all_sources_with_shared_fixture() {
    let runtime = common::make_embedded_service_runtime("explain-all-sources").await;
    let service = &runtime.service;

    let episode_id = common::ingest_episode(service, "explain-seed", "Alice promised a Friday launch.").await;
    let extracted = service.extract(&episode_id, None, None).await.unwrap();
    let fact_id = extracted.facts[0].fact_id.clone();

    let result = service
        .explain(memory_mcp::models::ExplainRequest {
            context_pack: vec![memory_mcp::models::ExplainItem {
                fact_id: Some(fact_id),
                source_episode: Some(episode_id),
                quote: Some("Alice promised a Friday launch.".to_string()),
                ..Default::default()
            }],
        })
        .await
        .unwrap();

    assert!(!result[0].all_sources.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test explain_populates_all_sources_with_shared_fixture --test explain_provenance -- --exact --nocapture`
Expected: FAIL with the current provenance assertion or fixture/resource collision behavior

- [ ] **Step 3: Write the minimal implementation**

```rust
// In tests/common/mod.rs
pub struct EmbeddedServiceRuntime {
    pub service: MemoryService,
    pub db_client: Arc<SurrealDbClient>,
    _temp_dir: tempfile::TempDir,
}

pub async fn make_embedded_service_runtime(test_name: &str) -> EmbeddedServiceRuntime {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
        "team".to_string(),
        "private-domain".to_string(),
    ];
    let db_name = format!("{}_{}", test_name, std::process::id());
    let db_client = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
            .await
            .expect("connect in-memory runtime"),
    );
    for namespace in &namespaces {
        db_client.apply_migrations(namespace).await.expect("apply migrations");
    }
    let service = MemoryService::new(db_client.clone(), namespaces, "warn".to_string(), 50, 100)
        .expect("service init");
    EmbeddedServiceRuntime {
        service,
        db_client,
        _temp_dir: temp_dir,
    }
}
```

```rust
// In src/service/triple_extractor.rs
// Remove NoOpTripleExtractor entirely once no production or test callsites remain.
// Keep RuleBasedTripleExtractor as the only concrete extractor.
```

```rust
// In src/cli/runtime.rs
// Remove deprecated public re-exports once all in-repo callsites are gone:
// pub use RunMode;
// pub use parse_cli_args;
```

```md
<!-- In README.md -->
## Experimental MCP Apps

Optional app workflows are behind `--features mcp-apps`.

```bash
cargo run --features mcp-apps -- serve
```

Canonical tools (`ingest`, `extract`, `resolve`, `assemble_context`, `explain`, `invalidate`) remain available without this feature.
```

- [ ] **Step 4: Run tests and quality gate**

Run: `cargo test --test explain_provenance -- --nocapture`
Expected: PASS

Run: `cargo check --all-targets`
Expected: PASS

Run: `cargo check --all-targets --all-features`
Expected: PASS

Run: `cargo clippy --all-targets`
Expected: PASS with zero warnings

Run: `cargo fmt --all --check`
Expected: PASS

Run: `cargo test`
Expected: PASS with the previously ignored lifecycle/provenance coverage now enabled

- [ ] **Step 5: Commit**

```bash
git add tests/common/mod.rs tests/explain_provenance.rs tests/lifecycle_archival.rs tests/lifecycle_decay.rs src/service/triple_extractor.rs src/cli/runtime.rs src/models.rs README.md
git commit -m "chore: stabilize critical tests and remove dead compatibility code"
```

## Self-Review

**1. Spec coverage**

- Dangling / non-wired ingestion review commit: covered by Tasks 2 and 3
- Fake diff payload and empty exports: covered by Task 2 and Task 3
- Direct handler DB mutations for lifecycle actions: covered by Tasks 2, 3, and 4
- Decorative session TTL: covered by Task 3
- Scope discipline gap (`team`, `private-domain`, silent fallback): covered by Task 1
- Lifecycle default drift (`30/0.35/180` vs `90/0.3/365`): covered by Tasks 1 and 4
- Bi-temporal archival mismatch: covered by Task 4
- Ignored failing lifecycle/provenance suites: covered by Task 5
- Dead code / duplicate helpers / deprecated compatibility surface: covered by Tasks 1 and 5

No gaps found against the audit findings that motivated this plan.

**2. Placeholder scan**

- No `TODO`, `TBD`, or “implement later” placeholders remain
- Every task includes a concrete failing test, implementation sketch, verification command, and commit
- Every created interface is named before later tasks consume it

**3. Type consistency**

- `MemoryScope`, `LifecyclePolicy`, `PrepareIngestionReviewRequest`, `CommitIngestionReviewRequest`, `DiffRequest`, and `SessionManager::get_valid()` are introduced once and reused consistently
- MCP app tasks depend on the service-layer use cases from Task 2 rather than inventing alternative names later
- Temporal-query changes in Task 4 rely on the same shared visibility clause instead of duplicating archival-specific SQL
