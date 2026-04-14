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

        Ok(Self {
            provider,
            model,
            model_dir: env::var("NER_MODEL_DIR").ok(),
            labels,
            threshold,
            batch_size,
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
