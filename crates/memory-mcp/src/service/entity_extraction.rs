//! Pluggable entity extraction abstractions.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use crate::models::EntityCandidate;

use super::MemoryError;

mod anno;
mod anno_onnx;
mod classifier;
mod gliner;
mod lfm2_gliner;
mod regex;

pub use anno::AnnoEntityExtractor;
pub use gliner::GlinerEntityExtractor;
// The concrete VAGO extractor type is publicly re-exported from this module.
// `service.rs` will surface it in a later step, so the lint cannot see a consumer yet.
#[allow(unused_imports)]
pub use lfm2_gliner::VagoLfm2EntityExtractor;
pub use regex::RegexEntityExtractor;
pub(crate) use unavailable::UnavailableEntityExtractor;

mod unavailable;

/// How an entity extractor must be scheduled by the episode pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NerScheduling {
    /// The extractor is async-safe and may run on the current runtime task.
    Inline,
    /// The extractor performs synchronous CPU/accelerator work and must run on
    /// Tokio's blocking pool.
    BlockingPool,
}

/// Extracts entity candidates from text.
#[async_trait]
pub trait EntityExtractor: Send + Sync {
    /// Human-readable provider name.
    fn provider_name(&self) -> &'static str {
        "unknown"
    }

    /// Declares where extraction must execute.
    fn scheduling(&self) -> NerScheduling;

    /// Returns the extractor's durable identity fingerprint.
    ///
    /// Lightweight extractors (Anno, regex) report selector + backend only;
    /// model-backed extractors add repository, revision, artifact identity,
    /// labels, threshold, and validation status. Persisted with new
    /// extraction projections so historical outputs stay attributable.
    fn fingerprint(&self) -> ExtractorFingerprint {
        ExtractorFingerprint {
            selector: self.provider_name().to_string(),
            backend: self.provider_name().to_string(),
            repository: None,
            revision: None,
            artifact_identity: None,
            labels: Vec::new(),
            threshold: None,
            revision_status: None,
            validation_status: None,
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            effective_device: None,
        }
    }

    /// Returns normalized entity candidates discovered in the supplied content.
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError>;

    /// Returns normalized entity candidates with custom zero-shot labels.
    ///
    /// Default implementation falls back to standard `extract_candidates`.
    /// Override in implementations that support custom label sets (e.g., GLiNER).
    async fn extract_candidates_with_labels(
        &self,
        content: &str,
        _zero_shot_labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        let _ = _zero_shot_labels;
        self.extract_candidates(content).await
    }
}

/// Type alias for the pluggable extraction function used by [`LlmEntityExtractor`].
///
/// Takes raw text and returns extracted entity candidates. Implementations
/// should call out to an LLM, gRPC service, or any other async backend.
type ExtractFuture =
    Pin<Box<dyn Future<Output = Result<Vec<EntityCandidate>, MemoryError>> + Send>>;

pub type ExtractFn = dyn Fn(String) -> ExtractFuture + Send + Sync;

/// LLM-backed entity extractor that delegates to a pluggable function.
///
/// This is a code-injected extension seam — the extraction function is
/// supplied via [`LlmEntityExtractor::new`] at construction time, so no HTTP
/// client or other transport is bundled. It is intentionally absent from
/// [`NerExtractorKind`] / the backend registry: there is no
/// `ENTITY_EXTRACTOR=llm` config flag and no `NerExtractorKind::Llm` variant.
/// Programmatic users (e.g. the eval harness) construct it directly. See
/// ADR-0029 (`docs/adr/0029-registry-of-pluggable-ner-backends.md`).
///
/// Errors from the injected function are propagated to the caller; there is
/// no silent fallback to an empty candidate list.
pub struct LlmEntityExtractor {
    extract_fn: Box<ExtractFn>,
}

impl std::fmt::Debug for LlmEntityExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmEntityExtractor").finish()
    }
}

impl LlmEntityExtractor {
    /// Creates a new LLM-backed extractor with the given extraction function.
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<EntityCandidate>, MemoryError>> + Send + 'static,
    {
        Self {
            extract_fn: Box::new(move |content| Box::pin(f(content))),
        }
    }
}

#[async_trait]
impl EntityExtractor for LlmEntityExtractor {
    fn provider_name(&self) -> &'static str {
        "llm"
    }

    fn scheduling(&self) -> NerScheduling {
        NerScheduling::Inline
    }

    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        (self.extract_fn)(content.to_string()).await
    }
}

/// Durable identity of the extractor that produced an entity projection.
///
/// Persisted alongside new extraction projections so historical outputs stay
/// attributable to the exact selector, backend, revision, and validation state.
/// Lightweight extractors leave model fields `None`; model-backed extractors
/// fill repository/revision/identity/validation/device.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExtractorFingerprint {
    /// Public selector (`NER_EXTRACTOR` value), e.g. `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`.
    pub selector: String,
    /// Stable backend name, e.g. `sauerkraut-lfm2.5-gliner`.
    pub backend: String,
    /// Artifact repository for model-backed extractors.
    pub repository: Option<String>,
    /// Resolved upstream revision (commit hash) when applicable.
    pub revision: Option<String>,
    /// Stable content identity over sorted `path:size:sha256` entries.
    pub artifact_identity: Option<String>,
    /// Normalized ordered labels.
    pub labels: Vec<String>,
    /// Effective confidence threshold.
    pub threshold: Option<f64>,
    /// How the revision was resolved at activation.
    pub revision_status: Option<crate::service::model_artifacts::RevisionStatus>,
    /// How the revision was validated.
    pub validation_status: Option<crate::service::model_artifacts::ValidationStatus>,
    /// Runtime/model-family version.
    pub runtime_version: String,
    /// Effective device (`cpu`/`metal`) — never the requested device alone.
    pub effective_device: Option<String>,
}

/// Future returned by every backend `build` hook.
pub(crate) type BackendBoxFuture =
    Pin<Box<dyn Future<Output = Result<Arc<dyn EntityExtractor>, MemoryError>> + Send>>;

/// Shared build inputs available to every backend constructor.
pub(crate) struct NerBuildContext {
    pub(crate) data_dir: std::path::PathBuf,
    pub(crate) logger: crate::logging::StdoutLogger,
    /// Model-progress sink selected by the hosting process: JSON lines on
    /// stderr for MCP stdio, human-readable lines on stderr for the CLI.
    pub(crate) progress: std::sync::Arc<dyn crate::service::model_artifacts::ModelProgressSink>,
}

impl Clone for NerBuildContext {
    fn clone(&self) -> Self {
        Self {
            data_dir: self.data_dir.clone(),
            logger: self.logger.clone(),
            progress: self.progress.clone(),
        }
    }
}

/// Signature of a backend construction hook.
pub(crate) type BackendBuildFn =
    fn(crate::config::NerExtractorConfig, NerBuildContext) -> BackendBoxFuture;

/// One registered NER backend: its catalog kind, stable log name, and builder.
struct BackendSpec {
    kind: crate::config::NerExtractorKind,
    name: &'static str,
    scheduling: fn() -> NerScheduling,
    build: BackendBuildFn,
}

/// Static registry of all `NER_EXTRACTOR`-selectable backends.
///
/// The LLM extractor is injected by code rather than by configuration, so it
/// is intentionally absent. Adding a backend: implement `EntityExtractor` and
/// a `build` hook in a new module, extend `NerExtractorKind` and
/// `NerExtractorConfig`, and append exactly one entry here.
fn backend_registry() -> &'static [BackendSpec] {
    use crate::config::NerExtractorKind as Kind;
    &[
        BackendSpec {
            kind: Kind::Anno,
            name: "anno",
            scheduling: anno::scheduling,
            build: anno::build,
        },
        BackendSpec {
            kind: Kind::Regex,
            name: "regex",
            scheduling: regex::scheduling,
            build: regex::build,
        },
        BackendSpec {
            kind: Kind::AnnoOnnx,
            name: "anno-onnx",
            scheduling: anno_onnx::scheduling,
            build: anno_onnx::build,
        },
        BackendSpec {
            kind: Kind::ClassicGliner,
            name: "gliner",
            scheduling: gliner::scheduling,
            build: gliner::build,
        },
        BackendSpec {
            kind: Kind::SauerkrautLfm25,
            name: "sauerkraut-lfm2.5-gliner",
            scheduling: lfm2_gliner::scheduling,
            build: lfm2_gliner::build,
        },
    ]
}

/// Factory function to create an entity extractor from NER configuration.
///
/// Dispatches through [`backend_registry`] to the selected backend's `build`
/// hook. This is the only extractor-dispatch point; no other code matches on
/// `NerExtractorKind`.
///
/// # Errors
///
/// Returns an error when the selected extractor is unknown to the registry or
/// the backend's own construction fails (invalid config, model load failure).
/// Factory function to create an entity extractor from NER configuration.
///
/// Dispatches through [`backend_registry`] to the selected backend's `build`
/// hook. This is the only extractor-dispatch point; no other code matches on
/// `NerExtractorKind`. Uses the human-readable CLI progress sink; MCP stdio
/// processes should call [`create_entity_extractor_with_progress`] with the
/// JSON-lines sink instead.
///
/// # Errors
///
/// Returns an error when the selected extractor is unknown to the registry or
/// the backend's own construction fails (invalid config, model load failure).
pub async fn create_entity_extractor(
    config: &crate::config::NerConfig,
    data_dir: &str,
    logger: &crate::logging::StdoutLogger,
) -> Result<Arc<dyn EntityExtractor>, MemoryError> {
    create_entity_extractor_with_progress(
        config,
        data_dir,
        logger,
        std::sync::Arc::new(crate::service::model_artifacts::CliProgressSink::new()),
    )
    .await
}

/// Extractor factory with an explicit model-progress sink.
///
/// The sink choice is a process concern: MCP stdio must keep stdout
/// JSON-RPC-only, so [`crate::service::model_artifacts::JsonLineProgressSink`]
/// writes to stderr; interactive CLI paths use
/// [`crate::service::model_artifacts::CliProgressSink`].
pub(crate) async fn create_entity_extractor_with_progress(
    config: &crate::config::NerConfig,
    data_dir: &str,
    logger: &crate::logging::StdoutLogger,
    progress: std::sync::Arc<dyn crate::service::model_artifacts::ModelProgressSink>,
) -> Result<Arc<dyn EntityExtractor>, MemoryError> {
    let kind = config.extractor.kind();
    let spec = backend_registry()
        .iter()
        .find(|spec| spec.kind == kind)
        .ok_or_else(|| {
            let known: Vec<&str> = backend_registry().iter().map(|s| s.name).collect();
            MemoryError::ConfigInvalid(format!(
                "unsupported NER extractor: {:?} (registered: {})",
                kind,
                known.join(", ")
            ))
        })?;
    let context = NerBuildContext {
        data_dir: std::path::PathBuf::from(data_dir),
        logger: logger.clone(),
        progress,
    };
    let extractor = (spec.build)(config.extractor.clone(), context).await?;
    let declared = (spec.scheduling)();
    if extractor.scheduling() != declared {
        return Err(MemoryError::ConfigInvalid(format!(
            "NER backend `{}` scheduling declaration disagrees with its registry entry: backend={:?}, registry={:?}",
            spec.name,
            extractor.scheduling(),
            declared,
        )));
    }
    Ok(extractor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn regex_entity_extractor_returns_deterministic_candidates() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Alice Smith met Bob Jones at Acme Inc")
            .await
            .unwrap();

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].canonical_name, "Acme Inc");
        assert_eq!(candidates[1].canonical_name, "Alice Smith");
        assert_eq!(candidates[2].canonical_name, "Bob Jones");
    }

    #[tokio::test]
    async fn regex_entity_extractor_includes_single_token_camel_case_names() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates(
                "OpenAI partnered with Anthropic while PostgreSQL backed Alice Smith",
            )
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "Alice Smith".to_string(),
                "Anthropic".to_string(),
                "OpenAI".to_string(),
                "PostgreSQL".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn regex_entity_extractor_filters_out_short_words() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("I met Bob at OpenAI on Monday at San Francisco")
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        // Should NOT include: I, At, In, On (1-2 letter words)
        // Should include: Bob, OpenAI, Monday, San Francisco (3+ chars)
        assert!(!names.contains(&"I".to_string()));
        assert!(!names.contains(&"At".to_string()));
        assert!(!names.contains(&"On".to_string()));

        assert!(names.contains(&"Bob".to_string()));
        assert!(names.contains(&"OpenAI".to_string()));
        assert!(names.contains(&"Monday".to_string()));
        assert!(names.contains(&"San Francisco".to_string()));
    }

    #[tokio::test]
    async fn regex_entity_extractor_supports_unicode_names() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Иван Петров встретился с Maria Garcia в компании TechCorp")
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        // Should include Cyrillic and Latin names
        assert!(names.contains(&"Иван Петров".to_string()));
        assert!(names.contains(&"Maria Garcia".to_string()));
        assert!(names.contains(&"TechCorp".to_string()));
    }

    #[tokio::test]
    async fn regex_entity_extractor_classifies_company_types() {
        let extractor = RegexEntityExtractor::new().unwrap();
        // Use company names that the regex can extract (multi-word or with lowercase)
        let candidates = extractor
            .extract_candidates("Acme Corp and Globex Inc and Initech Limited")
            .await
            .unwrap();

        for candidate in &candidates {
            assert_eq!(
                candidate.entity_type, "company",
                "{:?} should be classified as company",
                candidate.canonical_name
            );
        }
    }

    #[tokio::test]
    async fn regex_entity_extractor_classifies_event_types() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Tech Summit in San Francisco")
            .await
            .unwrap();

        let types: std::collections::HashMap<_, _> = candidates
            .iter()
            .map(|c| (c.canonical_name.as_str(), c.entity_type.as_str()))
            .collect();

        // "Tech Summit" contains the "Summit" indicator → classified as event
        assert_eq!(types.get("Tech Summit"), Some(&"event"));

        // "San Francisco" is in the gazetteer → classified as location
        assert_eq!(types.get("San Francisco"), Some(&"location"));
    }

    #[tokio::test]
    async fn regex_entity_extractor_classifies_person_types() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Alice Smith met Bob Jones")
            .await
            .unwrap();

        let types: std::collections::HashMap<_, _> = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.canonical_name.as_str(),
                    candidate.entity_type.as_str(),
                )
            })
            .collect();

        // Multi-word names without company suffixes are classified as person
        assert_eq!(types.get("Alice Smith"), Some(&"person"));
        assert_eq!(types.get("Bob Jones"), Some(&"person"));
    }

    #[tokio::test]
    async fn regex_entity_extractor_classifies_technology_types() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("PostgreSQL integrates with OpenAI")
            .await
            .unwrap();

        let types: std::collections::HashMap<_, _> = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.canonical_name.as_str(),
                    candidate.entity_type.as_str(),
                )
            })
            .collect();

        // Single-word CamelCase names are classified as technology
        assert_eq!(types.get("PostgreSQL"), Some(&"technology"));
        assert_eq!(types.get("OpenAI"), Some(&"technology"));
    }

    #[tokio::test]
    async fn llm_extractor_delegates_to_provided_function() {
        let extractor = LlmEntityExtractor::new(|_content| async move {
            Ok(vec![
                EntityCandidate {
                    entity_type: "person".into(),
                    canonical_name: "Alice Smith".into(),
                    aliases: vec![],
                },
                EntityCandidate {
                    entity_type: "company".into(),
                    canonical_name: "Acme Corp".into(),
                    aliases: vec![],
                },
            ])
        });

        let candidates = extractor
            .extract_candidates("irrelevant input")
            .await
            .unwrap();

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].canonical_name, "Alice Smith");
        assert_eq!(candidates[1].entity_type, "company");
    }

    #[tokio::test]
    async fn create_entity_extractor_defaults_to_anno() {
        let logger = crate::logging::StdoutLogger::new("warn");
        let extractor = create_entity_extractor(
            &crate::config::NerConfig::default(),
            "/tmp/memory-mcp-tests",
            &logger,
        )
        .await
        .expect("default extractor");

        assert_eq!(extractor.provider_name(), "anno");
    }

    #[test]
    fn registry_has_one_spec_per_extractor_kind() {
        let seen: std::collections::BTreeSet<_> =
            backend_registry().iter().map(|spec| spec.kind).collect();
        let expected: std::collections::BTreeSet<_> = [
            crate::config::NerExtractorKind::Anno,
            crate::config::NerExtractorKind::Regex,
            crate::config::NerExtractorKind::AnnoOnnx,
            crate::config::NerExtractorKind::ClassicGliner,
            crate::config::NerExtractorKind::SauerkrautLfm25,
        ]
        .into_iter()
        .collect();
        assert_eq!(backend_registry().len(), 5);
        assert_eq!(seen, expected);
    }

    #[test]
    fn registry_names_are_stable() {
        let names: Vec<&str> = backend_registry().iter().map(|spec| spec.name).collect();
        assert_eq!(
            names,
            vec![
                "anno",
                "regex",
                "anno-onnx",
                "gliner",
                "sauerkraut-lfm2.5-gliner",
            ]
        );
    }

    #[test]
    fn registry_declares_scheduling_for_every_backend() {
        let scheduling: Vec<NerScheduling> = backend_registry()
            .iter()
            .map(|spec| (spec.scheduling)())
            .collect();
        assert_eq!(
            scheduling,
            vec![
                NerScheduling::Inline,
                NerScheduling::Inline,
                NerScheduling::BlockingPool,
                NerScheduling::BlockingPool,
                NerScheduling::BlockingPool,
            ]
        );
    }

    #[tokio::test]
    async fn registry_dispatches_to_regex_backend() {
        let logger = crate::logging::StdoutLogger::new("error");
        let config = crate::config::NerConfig {
            extractor: crate::config::NerExtractorConfig::Regex,
        };
        let extractor = create_entity_extractor(&config, "/tmp/memory-mcp-tests", &logger)
            .await
            .expect("regex extractor");
        assert_eq!(extractor.provider_name(), "regex");
    }

    #[tokio::test]
    async fn anno_onnx_backend_fails_without_model_files() {
        let logger = crate::logging::StdoutLogger::new("error");
        let config = crate::config::NerConfig {
            extractor: crate::config::NerExtractorConfig::AnnoOnnx(
                crate::config::ModelBackedNerConfig {
                    cache_dir: Some(std::path::PathBuf::from("/nonexistent/anno-onnx-model")),
                    labels: vec!["person".to_string()],
                    threshold: None,
                    max_concurrency: 1,
                    idle_unload_secs: 0,
                },
            ),
        };
        let result = create_entity_extractor(&config, "/tmp/memory-mcp-tests", &logger).await;
        match result {
            Err(MemoryError::ConfigInvalid(message)) => {
                assert!(
                    message.contains("not found"),
                    "expected missing-model guidance, got: {message}"
                );
            }
            Err(other) => panic!("expected ConfigInvalid, got {other}"),
            Ok(_) => panic!("anno-onnx backend must fail without model files"),
        }
    }

    #[test]
    fn gliner_span_event_has_stable_operation_name() {
        let event = crate::service::entity_extraction::gliner::build_span_scoring_log_event(
            12,
            72,
            std::time::Duration::from_millis(7),
        );
        assert_eq!(
            event["op"],
            serde_json::json!("ner.gliner.span_scores.done")
        );
        assert_eq!(event["args"]["text_words"], serde_json::json!(12));
        assert_eq!(event["result"]["span_count"], serde_json::json!(72));
    }

    #[test]
    fn gliner_batch_event_reports_actual_and_configured_bounds() {
        let event = crate::service::entity_extraction::gliner::build_batching_log_event(
            7, 3, 4, 1440, 1536,
        );
        assert_eq!(event["op"], serde_json::json!("ner.gliner.batching.done"));
        assert_eq!(event["args"]["window_count"], serde_json::json!(7));
        assert_eq!(
            event["args"]["configured_max_padded_tokens"],
            serde_json::json!(1536)
        );
        assert_eq!(event["result"]["batch_count"], serde_json::json!(3));
        assert_eq!(event["result"]["largest_batch"], serde_json::json!(4));
        assert_eq!(
            event["result"]["max_padded_tokens"],
            serde_json::json!(1440)
        );
    }
}
