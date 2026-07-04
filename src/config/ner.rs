//! NER (Named Entity Recognition) configuration.

use std::env;
use std::path::PathBuf;

use super::constants::*;
use super::helpers::parse_env;
use crate::service::MemoryError;

/// Supported NER provider kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NerProviderKind {
    /// Regex heuristic fallback.
    Regex,
    /// In-process anno-backed NER.
    Anno,
    /// Local GLiNER model executed via Candle.
    LocalGliner,
}

/// Configuration for NER entity extraction provider selection.
#[derive(Debug, Clone)]
pub struct NerConfig {
    /// Which provider to use.
    pub provider: NerProviderKind,
    /// HuggingFace repo ID for the GLiNER model.
    pub model: Option<String>,
    /// Optional override for model cache directory.
    pub model_dir: Option<String>,
    /// Entity labels to extract.
    pub labels: Vec<String>,
    /// Confidence threshold for accepted spans.
    pub threshold: f64,
    /// Batch size for inference.
    pub batch_size: usize,
    /// Max padded tokens per batch.
    pub max_batch_tokens: usize,
    /// Max concurrent local NER inference operations.
    pub max_concurrency: usize,
}

impl Default for NerConfig {
    fn default() -> Self {
        Self {
            provider: NerProviderKind::Anno,
            model: None,
            model_dir: None,
            labels: default_ner_labels(),
            threshold: DEFAULT_NER_THRESHOLD,
            batch_size: DEFAULT_NER_BATCH_SIZE,
            max_batch_tokens: DEFAULT_NER_MAX_BATCH_TOKENS,
            max_concurrency: DEFAULT_NER_MAX_CONCURRENCY,
        }
    }
}

impl NerConfig {
    /// Loads NER provider configuration from environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::ConfigInvalid`] for unsupported providers or
    /// malformed numeric settings.
    pub fn from_env() -> Result<Self, MemoryError> {
        let provider = match env::var("NER_PROVIDER")
            .unwrap_or_else(|_| "anno".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "regex" => NerProviderKind::Regex,
            "anno" => NerProviderKind::Anno,
            "local-gliner" | "local_gliner" | "gliner" => NerProviderKind::LocalGliner,
            other => {
                return Err(MemoryError::ConfigInvalid(format!(
                    "unsupported NER_PROVIDER `{other}`"
                )));
            }
        };

        let model = match provider {
            NerProviderKind::LocalGliner => Some(
                env::var("NER_MODEL").unwrap_or_else(|_| "urchade/gliner_multi-v2.1".to_string()),
            ),
            _ => env::var("NER_MODEL").ok(),
        };

        let labels = env::var("NER_LABELS")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_else(|_| default_ner_labels());

        let threshold = parse_env::<f64>("NER_THRESHOLD")?.unwrap_or(DEFAULT_NER_THRESHOLD);
        let batch_size = parse_env::<usize>("NER_BATCH_SIZE")?.unwrap_or(DEFAULT_NER_BATCH_SIZE);
        let max_batch_tokens =
            parse_env::<usize>("NER_MAX_BATCH_TOKENS")?.unwrap_or(DEFAULT_NER_MAX_BATCH_TOKENS);
        let max_concurrency =
            parse_env::<usize>("NER_MAX_CONCURRENCY")?.unwrap_or(DEFAULT_NER_MAX_CONCURRENCY);

        if batch_size == 0 || max_batch_tokens == 0 || max_concurrency == 0 {
            return Err(MemoryError::ConfigInvalid(
                "NER_BATCH_SIZE, NER_MAX_BATCH_TOKENS, and NER_MAX_CONCURRENCY must be greater than zero"
                    .to_string(),
            ));
        }

        Ok(Self {
            provider,
            model,
            model_dir: env::var("NER_MODEL_DIR").ok(),
            labels,
            threshold,
            batch_size,
            max_batch_tokens,
            max_concurrency,
        })
    }

    /// Resolves the model directory path for local GLiNER providers.
    ///
    /// Resolution order:
    /// 1. `NER_MODEL_DIR` env var
    /// 2. `<data_dir>/models/ner/<model_name>/`
    #[must_use]
    pub fn model_dir_or_default(&self, data_dir: &str) -> String {
        self.model_dir.clone().unwrap_or_else(|| {
            let model_name = self.model.as_deref().unwrap_or("urchade/gliner_multi-v2.1");
            let sanitized = model_name.replace('/', "--");

            PathBuf::from(data_dir)
                .join("models")
                .join("ner")
                .join(sanitized)
                .to_string_lossy()
                .to_string()
        })
    }
}

fn default_ner_labels() -> Vec<String> {
    vec![
        "person".to_string(),
        "company".to_string(),
        "location".to_string(),
        "product".to_string(),
        "event".to_string(),
        "technology".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_ner_env(vars: &[(&str, Option<&str>)], test: impl FnOnce()) {
        let _guard = env_lock().lock().expect("NER env lock");
        let saved = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for (key, value) in vars {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
        for (key, value) in saved {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        outcome.expect("NER config test body");
    }

    #[test]
    fn ner_runtime_limits_have_safe_defaults() {
        with_ner_env(
            &[
                ("NER_BATCH_SIZE", None),
                ("NER_MAX_BATCH_TOKENS", None),
                ("NER_MAX_CONCURRENCY", None),
            ],
            || {
                let config = NerConfig::from_env().expect("default NER config");
                assert_eq!(config.batch_size, 4);
                assert_eq!(config.max_batch_tokens, 1536);
                assert_eq!(config.max_concurrency, 1);
            },
        );
    }

    #[test]
    fn ner_runtime_limits_reject_zero() {
        for key in [
            "NER_BATCH_SIZE",
            "NER_MAX_BATCH_TOKENS",
            "NER_MAX_CONCURRENCY",
        ] {
            with_ner_env(&[(key, Some("0"))], || {
                assert!(matches!(
                    NerConfig::from_env(),
                    Err(MemoryError::ConfigInvalid(_))
                ));
            });
        }
    }
}
