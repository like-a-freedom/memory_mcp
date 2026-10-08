//! Memory bounded context — public interface.
//!
//! Memory owns episode ingestion, recall and assembly,
//! explanation, lifecycle and procedures. Source episodes and
//! provenance survive derived-knowledge changes.
//!
//! Use cases here take the narrow ports a consumer actually
//! needs rather than a shared service container: an ingest
//! caller needs [`IngestionPort`] and nothing else, and a
//! resolve caller needs [`EntityResolutionPort`] plus
//! [`RateLimitPort`]. The policy that a refused caller never
//! reaches the resolver lives in the use case, not in a
//! container.

use chrono::{DateTime, Utc};

use crate::error::MemoryError;
use crate::memory::lifecycle_workers::LifecycleHandles;
use crate::models::{AssembledContextItem, ExtractResult};

/// Episode ingestion dependency.
#[async_trait::async_trait]
pub trait IngestionPort: Send + Sync {
    /// Persist source material as an episode and return its ID.
    ///
    /// The whole request travels through the port, including the
    /// caller identity, because the rate-limit charge is part of
    /// ingestion and must not be duplicated by a caller that also
    /// wants the access policy applied.
    async fn ingest_episode(
        &self,
        request: crate::models::IngestRequest,
        access: Option<crate::models::AccessPayload>,
    ) -> Result<String, MemoryError>;
}

/// Token-bucket access policy.
pub trait RateLimitPort: Send + Sync {
    /// Charge one access to `caller`, or refuse it.
    fn check(&self, caller: Option<&str>) -> Result<(), MemoryError>;
}

/// Knowledge-owned entity resolution, injected into memory.
#[async_trait::async_trait]
pub trait EntityResolutionPort: Send + Sync {
    /// Resolve a candidate to a canonical entity ID, reporting
    /// whether the entity was created.
    async fn resolve_or_create(
        &self,
        candidate: crate::models::EntityCandidate,
    ) -> Result<(String, bool), MemoryError>;
}

/// Resolve one entity candidate.
#[derive(Debug, Clone)]
pub struct ResolveCommand {
    pub candidate: crate::models::EntityCandidate,
    pub caller_id: Option<String>,
}

/// Resolve an entity candidate through the injected ports.
///
/// The rate limit is charged first, so a refused caller never
/// reaches the resolver and never triggers a write.
pub async fn resolve_entity(
    resolver: &(impl EntityResolutionPort + ?Sized),
    rate_limit: &(impl RateLimitPort + ?Sized),
    command: &ResolveCommand,
) -> Result<(String, bool), MemoryError> {
    rate_limit.check(command.caller_id.as_deref())?;
    resolver.resolve_or_create(command.candidate.clone()).await
}

/// Ingest one episode through the injected port.
///
/// The rate-limit charge lives inside the ingestion port rather than
/// here, so a caller that also enforces the access policy would
/// debit the shared bucket twice.
pub async fn ingest_episode(
    ingestion: &(impl IngestionPort + ?Sized),
    request: crate::models::IngestRequest,
    access: Option<crate::models::AccessPayload>,
) -> Result<String, MemoryError> {
    ingestion.ingest_episode(request, access).await
}

/// Recall dependency: the multi-tier retrieval pipeline.
#[async_trait::async_trait]
pub trait ContextRetrievalPort: Send + Sync {
    /// Retrieve the context items for a query.
    async fn retrieve(
        &self,
        command: &RecallCommand,
    ) -> Result<Vec<AssembledContextItem>, MemoryError>;
}

/// Assemble context for one query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallCommand {
    pub query: String,
    pub budget: i32,
    pub caller_id: Option<String>,
}

/// Recall context through the injected retrieval port.
///
/// The access policy is charged first, so a rate-limited caller
/// never runs the (read-heavy) retrieval pipeline.
pub async fn recall_context(
    retrieval: &(impl ContextRetrievalPort + ?Sized),
    rate_limit: &(impl RateLimitPort + ?Sized),
    command: &RecallCommand,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    rate_limit.check(command.caller_id.as_deref())?;
    retrieval.retrieve(command).await
}

/// Extraction dependency: entities, facts and relationships from
/// a stored episode.
#[async_trait::async_trait]
pub trait EpisodeExtractionPort: Send + Sync {
    /// Whether the episode exists.
    async fn episode_exists(&self, episode_id: &str) -> Result<bool, MemoryError>;

    /// Extract from a known-present episode, returning the result
    /// together with the source episode the caller needs for
    /// audit logging.
    async fn extract(&self, command: &ExtractCommand<'_>) -> Result<ExtractedEpisode, MemoryError>;
}

/// An extraction result plus the episode it was derived from.
#[derive(Debug, Clone, Default)]
pub struct ExtractedEpisode {
    /// The source episode, when the owner could load it.
    pub episode: Option<crate::models::Episode>,
    pub result: ExtractResult,
}

/// Extract entities and facts from one episode.
#[derive(Debug, Clone)]
pub struct ExtractCommand<'a> {
    pub episode_id: String,
    /// Caller-supplied NER labels; `None` uses the default
    /// extractor configuration.
    pub zero_shot_labels: Option<&'a [String]>,
    pub caller_id: Option<String>,
}

/// Extract from an episode through the injected port.
///
/// Order is deliberate: the access policy is charged, the
/// episode is verified, and only then is extraction run. A
/// refused caller or a missing episode never runs the (model
/// backed) extractor.
pub async fn extract_from_episode(
    extraction: &(impl EpisodeExtractionPort + ?Sized),
    rate_limit: &(impl RateLimitPort + ?Sized),
    command: &ExtractCommand<'_>,
) -> Result<ExtractedEpisode, MemoryError> {
    rate_limit.check(command.caller_id.as_deref())?;

    if !extraction.episode_exists(&command.episode_id).await? {
        return Err(MemoryError::NotFound(format!(
            "episode_id not found: {}",
            command.episode_id
        )));
    }

    extraction.extract(command).await
}

/// Whether a canonical record exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredRecord {
    Present,
    Absent,
}

/// Explanation dependency: provenance citation for assembled
/// context items.
#[async_trait::async_trait]
pub trait ExplanationPort: Send + Sync {
    /// Explain one context pack, citing sources.
    async fn explain(
        &self,
        request: crate::models::ExplainRequest,
    ) -> Result<Vec<crate::models::ExplainItem>, MemoryError>;
}

/// Explain a context pack through the injected port.
///
/// The rate limit is charged first so a refused caller cannot
/// trigger the (read-heavy) provenance pipeline.
pub async fn explain_context(
    explanation: &(impl ExplanationPort + ?Sized),
    rate_limit: &(impl RateLimitPort + ?Sized),
    request: crate::models::ExplainRequest,
    caller_id: Option<String>,
) -> Result<Vec<crate::models::ExplainItem>, MemoryError> {
    rate_limit.check(caller_id.as_deref())?;
    explanation.explain(request).await
}

/// Invalidate one fact.
///
/// `t_invalid` is the caller-supplied valid time; the
/// transaction time stays server-owned.
#[derive(Debug, Clone)]
pub struct InvalidationRequest {
    pub fact_id: String,
    pub t_invalid: DateTime<Utc>,
    pub reason: String,
    pub caller_id: Option<String>,
}

/// Invalidate one fact by closing its bi-temporal validity.
///
/// This is the single owner of the close decision: the port
/// carries the close out, and the policy that decides *what*
/// gets closed and in which order lives here.
#[async_trait::async_trait]
pub trait InvalidationPort: Send + Sync {
    /// Look up a record to verify it exists.
    async fn find_record(&self, record_id: &str) -> Result<StoredRecord, MemoryError>;

    /// Close both bi-temporal fields on a record.
    ///
    /// `t_invalid_ingested` is the server-owned transaction
    /// time; the caller supplies only valid time. A `None`
    /// transaction time means the owner stamps server-side
    /// now, which is the only correct default.
    async fn close_record(
        &self,
        record_id: &str,
        t_invalid: DateTime<Utc>,
        t_invalid_ingested: Option<DateTime<Utc>>,
        reason: &str,
    ) -> Result<(), MemoryError>;

    /// Close the claims derived from a fact.
    async fn close_claims_for_fact(&self, fact_id: &str) -> Result<(), MemoryError>;

    /// Whether a claim projection pipeline is wired for this
    /// runtime.
    fn claim_pipeline_is_wired(&self) -> bool;

    /// Drop the assembled-context cache.
    async fn invalidate_assembled_context(&self) -> Result<(), MemoryError>;
}

/// Invalidate one fact.
///
/// Order is deliberate and observable: rate limit, target kind,
/// record-id shape, existence check, close, derived-claim close,
/// cache invalidation.
///
/// The target kind is checked here rather than in a store: this
/// use case is what states that invalidation applies to facts, and
/// that invariant must hold no matter which store answers the
/// existence check. A record id naming any other kind is refused
/// before the close owner is reached, so a caller's string can
/// never select a different aggregate's table.
pub async fn invalidate_fact(
    port: &(impl InvalidationPort + ?Sized),
    rate_limit: &(impl RateLimitPort + ?Sized),
    request: &InvalidationRequest,
) -> Result<(), MemoryError> {
    rate_limit.check(request.caller_id.as_deref())?;

    if !request.fact_id.starts_with("fact:") {
        return Err(MemoryError::Validation(format!(
            "record_id '{}' is not a fact record id; expected the canonical 'fact:<id>' form",
            request.fact_id
        )));
    }
    crate::storage::validate_record_id(&request.fact_id)?;

    if port.find_record(&request.fact_id).await? == StoredRecord::Absent {
        return Err(MemoryError::NotFound("fact_id not found".into()));
    }

    port.close_record(
        &request.fact_id,
        request.t_invalid,
        // Transaction time is server-owned: the caller never
        // supplies it, so a stale client cannot backdate the
        // fact into the live view.
        None,
        &request.reason,
    )
    .await?;

    if port.claim_pipeline_is_wired() {
        port.close_claims_for_fact(&request.fact_id).await?;
    }
    port.invalidate_assembled_context().await?;
    Ok(())
}

/// Run one memory-owned confidence-decay pass using its narrow lifecycle handles.
pub async fn run_decay(
    handles: &LifecycleHandles<'_>,
    threshold: f64,
    half_life_days: f64,
) -> Result<usize, MemoryError> {
    crate::memory::lifecycle_workers::decay::run_decay_pass(handles, threshold, half_life_days)
        .await
}

/// Run one memory-owned episode archival pass using its narrow lifecycle handles.
pub async fn run_archival(
    handles: &LifecycleHandles<'_>,
    age_days: u32,
) -> Result<usize, MemoryError> {
    crate::memory::lifecycle_workers::archival::run_archival_pass(handles, age_days).await
}

/// Rebuild communities through the knowledge-owned graph store.
pub async fn rebuild_communities(handles: &LifecycleHandles<'_>) -> Result<usize, MemoryError> {
    crate::memory::lifecycle_workers::run_community_rebuild_pass(handles).await
}
