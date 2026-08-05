//! Pluggable entity extraction abstractions.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use crate::models::EntityCandidate;

use super::MemoryError;

mod anno;
mod classifier;
mod gliner;
mod regex;

pub use anno::AnnoEntityExtractor;
pub use gliner::GlinerEntityExtractor;
pub use regex::RegexEntityExtractor;

/// Extracts entity candidates from text.
#[async_trait]
pub trait EntityExtractor: Send + Sync {
    /// Human-readable provider name.
    fn provider_name(&self) -> &'static str {
        "unknown"
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
/// Activate via config flag `ENTITY_EXTRACTOR=llm`. The extraction function
/// is injected at construction time — no HTTP client dependency required.
/// Falls back gracefully: if the function returns an error, returns an
/// empty candidate list (the caller can retry with [`RegexEntityExtractor`]).
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

    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        (self.extract_fn)(content.to_string()).await
    }
}

/// Future returned by every backend `build` hook.
pub(crate) type BackendBoxFuture =
    Pin<Box<dyn Future<Output = Result<Arc<dyn EntityExtractor>, MemoryError>> + Send>>;

/// Signature of a backend construction hook.
pub(crate) type BackendBuildFn =
    fn(crate::config::NerConfig, String, crate::logging::StdoutLogger) -> BackendBoxFuture;

/// One registered NER backend: its config kind, stable log name, and builder.
struct BackendSpec {
    kind: crate::config::NerProviderKind,
    name: &'static str,
    build: BackendBuildFn,
}

/// Static registry of all config-selectable NER backends.
///
/// The LLM extractor is injected by code, not selected via `NerProviderKind`,
/// so it is intentionally absent. To add a backend: implement `EntityExtractor`
/// and a `build` hook in a new module, add a `NerProviderKind` variant, and
/// append one entry here.
fn backend_registry() -> &'static [BackendSpec] {
    use crate::config::NerProviderKind as Kind;
    &[
        BackendSpec {
            kind: Kind::Regex,
            name: "regex",
            build: regex::build,
        },
        BackendSpec {
            kind: Kind::Anno,
            name: "anno",
            build: anno::build,
        },
        BackendSpec {
            kind: Kind::LocalGliner,
            name: "gliner",
            build: gliner::build,
        },
    ]
}

/// Factory function to create an entity extractor from NER configuration.
///
/// Dispatches through [`backend_registry`] to the selected backend's `build`
/// hook. This is the only provider-dispatch point; no other code matches on
/// `NerProviderKind`.
///
/// # Errors
///
/// Returns an error when the selected provider is unknown to the registry or
/// the backend's own construction fails (invalid config, model load failure).
pub async fn create_entity_extractor(
    config: &crate::config::NerConfig,
    data_dir: &str,
    logger: &crate::logging::StdoutLogger,
) -> Result<Arc<dyn EntityExtractor>, MemoryError> {
    let spec = backend_registry()
        .iter()
        .find(|spec| spec.kind == config.provider)
        .ok_or_else(|| {
            let known: Vec<&str> = backend_registry().iter().map(|s| s.name).collect();
            MemoryError::ConfigInvalid(format!(
                "unsupported NER provider: {:?} (registered: {})",
                config.provider,
                known.join(", ")
            ))
        })?;
    (spec.build)(config.clone(), data_dir.to_string(), logger.clone()).await
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
    fn registry_has_one_spec_per_config_dispatchable_provider_kind() {
        // The LLM extractor is injected by code, not by config, so it has no
        // NerProviderKind and no registry entry.
        assert_eq!(backend_registry().len(), 3);
    }

    #[test]
    fn registry_names_match_backend_provider_names() {
        for spec in backend_registry() {
            assert!(matches!(spec.name, "regex" | "anno" | "gliner"));
        }
    }

    #[tokio::test]
    async fn registry_dispatches_to_regex_backend() {
        let logger = crate::logging::StdoutLogger::new("error");
        let mut config = crate::config::NerConfig::default();
        config.provider = crate::config::NerProviderKind::Regex;
        let extractor = create_entity_extractor(&config, "/tmp/memory-mcp-tests", &logger)
            .await
            .expect("regex extractor");
        assert_eq!(extractor.provider_name(), "regex");
    }

    #[tokio::test]
    async fn gliner_build_requires_model_name_when_provider_is_gliner() {
        let mut config = crate::config::NerConfig::default();
        config.provider = crate::config::NerProviderKind::LocalGliner;
        config.model = None;
        let logger = crate::logging::StdoutLogger::new("error");
        let result = create_entity_extractor(&config, "/tmp/memory-mcp-tests", &logger).await;
        match result {
            Err(MemoryError::ConfigInvalid(_)) => {}
            Err(other) => panic!("expected ConfigInvalid, got {other}"),
            Ok(_) => panic!("expected Err when gliner model is missing"),
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
