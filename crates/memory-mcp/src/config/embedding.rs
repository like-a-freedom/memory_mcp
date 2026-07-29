//! Embedding configuration.

use std::env;
use std::path::PathBuf;

use serde_json::json;

use super::constants::*;
use super::helpers::{parse_bool_env, parse_env};
use crate::service::MemoryError;

/// Supported embedding provider kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingProviderKind {
    /// Semantic retrieval is disabled.
    Disabled,
    /// Local Candle-based embedding provider.
    LocalCandle,
    /// OpenAI-compatible `/embeddings` endpoint.
    OpenAiCompatible,
    /// Ollama `/api/embeddings` endpoint.
    Ollama,
}

/// Configuration for optional embedding provider integration.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Which provider to use.
    pub provider: EmbeddingProviderKind,
    /// Provider base URL.
    pub base_url: Option<String>,
    /// Embedding model name.
    pub model: Option<String>,
    /// Optional API key for OpenAI-compatible providers.
    pub api_key: Option<String>,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Explicit operator-provided embedding dimension override.
    pub dimension_override: Option<usize>,
    /// Maximum input tokens for chunking.
    pub max_tokens: usize,
    /// Minimum cosine similarity required for semantic retrieval.
    pub similarity_threshold: f64,
    /// Optional override for model directory path (used by local providers).
    pub model_dir: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: EmbeddingProviderKind::Disabled,
            base_url: None,
            model: None,
            api_key: None,
            timeout_secs: DEFAULT_EMBEDDING_TIMEOUT_SECS,
            dimension_override: None,
            max_tokens: DEFAULT_EMBEDDING_MAX_TOKENS,
            similarity_threshold: DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            model_dir: None,
        }
    }
}

#[must_use]
pub fn build_embedding_signature(
    provider_label: &str,
    model: Option<&str>,
    base_url: Option<&str>,
    dimension: usize,
) -> String {
    use sha2::{Digest, Sha256};

    let material = json!({
        "provider": provider_label,
        "model": model,
        "base_url": base_url.map(|url| url.trim_end_matches('/')),
        "dimension": dimension,
    });

    let mut hasher = Sha256::new();
    hasher.update(material.to_string().as_bytes());
    format!("embsig:{}", hex::encode(hasher.finalize()))
}

impl EmbeddingConfig {
    /// Loads optional embedding provider configuration from environment variables.
    ///
    /// When disabled, the rest of the server keeps working without semantic retrieval.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::ConfigInvalid`] for invalid provider names or dimensions,
    /// and [`MemoryError::ConfigMissing`] when a required variable is absent while
    /// embeddings are enabled.
    pub fn from_env() -> Result<Self, MemoryError> {
        let enabled = parse_bool_env("EMBEDDINGS_ENABLED")
            .or_else(|| env::var("EMBEDDINGS_PROVIDER").ok().map(|_| true))
            .unwrap_or(false);
        let timeout_secs =
            parse_env::<u64>("EMBEDDINGS_TIMEOUT_SECS")?.unwrap_or(DEFAULT_EMBEDDING_TIMEOUT_SECS);
        let configured_dimension = parse_env::<usize>("SURREALDB_EMBEDDING_DIMENSION")?;
        let max_tokens =
            parse_env::<usize>("EMBEDDINGS_MAX_TOKENS")?.unwrap_or(DEFAULT_EMBEDDING_MAX_TOKENS);
        let similarity_threshold = parse_env::<f64>("EMBEDDINGS_SIMILARITY_THRESHOLD")?
            .unwrap_or(DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD);
        let model_dir = env::var("EMBEDDINGS_MODEL_DIR").ok();

        if !enabled {
            return Ok(Self {
                timeout_secs,
                dimension_override: configured_dimension,
                max_tokens,
                similarity_threshold,
                model_dir,
                ..Self::default()
            });
        }

        let provider = match env::var("EMBEDDINGS_PROVIDER")
            .unwrap_or_else(|_| "local-candle".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "local-candle" | "local_candle" | "localcandle" => EmbeddingProviderKind::LocalCandle,
            "openai" | "openai-compatible" | "openai_compatible" => {
                EmbeddingProviderKind::OpenAiCompatible
            }
            "ollama" => EmbeddingProviderKind::Ollama,
            other => {
                return Err(MemoryError::ConfigInvalid(format!(
                    "unsupported EMBEDDINGS_PROVIDER `{other}`"
                )));
            }
        };

        let model = match provider {
            EmbeddingProviderKind::LocalCandle => Some(
                env::var("EMBEDDINGS_MODEL")
                    .unwrap_or_else(|_| "intfloat/multilingual-e5-small".to_string()),
            ),
            _ => Some(
                env::var("EMBEDDINGS_MODEL")
                    .map_err(|_| MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string()))?,
            ),
        };

        let base_url = match provider {
            EmbeddingProviderKind::LocalCandle => None,
            EmbeddingProviderKind::OpenAiCompatible => Some(
                env::var("EMBEDDINGS_BASE_URL")
                    .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            ),
            EmbeddingProviderKind::Ollama => Some(
                env::var("EMBEDDINGS_BASE_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string()),
            ),
            EmbeddingProviderKind::Disabled => None,
        };

        Ok(Self {
            provider,
            base_url,
            model,
            api_key: env::var("EMBEDDINGS_API_KEY").ok(),
            timeout_secs,
            dimension_override: configured_dimension,
            max_tokens,
            similarity_threshold,
            model_dir,
        })
    }

    /// Returns true when semantic embeddings should be used.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !matches!(self.provider, EmbeddingProviderKind::Disabled)
    }

    /// Returns the canonical provider label used in logs and signatures.
    #[must_use]
    pub fn provider_label(&self) -> &'static str {
        match self.provider {
            EmbeddingProviderKind::Disabled => "disabled",
            EmbeddingProviderKind::LocalCandle => "local-candle",
            EmbeddingProviderKind::OpenAiCompatible => "openai-compatible",
            EmbeddingProviderKind::Ollama => "ollama",
        }
    }

    /// Returns the non-authoritative fallback dimension used outside resolved preflight paths.
    #[must_use]
    pub fn fallback_dimension(&self) -> usize {
        self.dimension_override.unwrap_or(match self.provider {
            EmbeddingProviderKind::LocalCandle => DEFAULT_LOCAL_CANDLE_EMBEDDING_DIMENSION,
            _ => DEFAULT_EMBEDDING_DIMENSION,
        })
    }

    /// Resolves the model directory path for local embedding providers.
    ///
    /// Resolution order:
    /// 1. `EMBEDDINGS_MODEL_DIR` env var
    /// 2. `<data_dir>/models/<model_name>/`
    #[must_use]
    pub fn model_dir_or_default(&self, data_dir: &str) -> String {
        self.model_dir.clone().unwrap_or_else(|| {
            let model_name = self
                .model
                .as_deref()
                .unwrap_or("intfloat/multilingual-e5-small");
            PathBuf::from(data_dir)
                .join("models")
                .join(model_name)
                .to_string_lossy()
                .to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_env_vars(vars: &[(&str, Option<&str>)], test: impl FnOnce()) {
        let _guard = env_lock().lock().expect("env lock");
        let saved = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect::<Vec<_>>();

        unsafe {
            for (key, value) in vars {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }

        let result = panic::catch_unwind(AssertUnwindSafe(test));

        unsafe {
            for (key, value) in saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }

        result.expect("test body should not panic");
    }

    #[test]
    fn embedding_signature_changes_when_model_changes() {
        let first = build_embedding_signature(
            "openai-compatible",
            Some("text-embedding-3-small"),
            Some("https://api.openai.com/v1"),
            1536,
        );
        let second = build_embedding_signature(
            "openai-compatible",
            Some("text-embedding-3-large"),
            Some("https://api.openai.com/v1"),
            1536,
        );

        assert_ne!(first, second);
    }

    #[test]
    fn embedding_signature_is_stable_for_equivalent_config() {
        let left = build_embedding_signature(
            "local-candle",
            Some("intfloat/multilingual-e5-small"),
            None,
            384,
        );
        let right = build_embedding_signature(
            "local-candle",
            Some("intfloat/multilingual-e5-small"),
            None,
            384,
        );

        assert_eq!(left, right);
    }

    #[test]
    fn embedding_config_from_env_preserves_dimension_override_when_set() {
        with_env_vars(
            &[
                ("EMBEDDINGS_ENABLED", Some("true")),
                ("EMBEDDINGS_PROVIDER", Some("local-candle")),
                ("EMBEDDINGS_MODEL", Some("intfloat/multilingual-e5-small")),
                ("SURREALDB_EMBEDDING_DIMENSION", Some("777")),
            ],
            || {
                let config = EmbeddingConfig::from_env().expect("config from env");
                assert_eq!(config.dimension_override, Some(777));
            },
        );
    }

    #[test]
    fn embedding_config_from_env_leaves_dimension_override_unset_when_absent() {
        with_env_vars(
            &[
                ("EMBEDDINGS_ENABLED", Some("true")),
                ("EMBEDDINGS_PROVIDER", Some("local-candle")),
                ("EMBEDDINGS_MODEL", Some("intfloat/multilingual-e5-small")),
                ("SURREALDB_EMBEDDING_DIMENSION", None),
            ],
            || {
                let config = EmbeddingConfig::from_env().expect("config from env");
                assert_eq!(config.dimension_override, None);
            },
        );
    }
}
