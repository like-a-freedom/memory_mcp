//! NER (Named Entity Recognition) configuration.

use std::env;
use std::path::PathBuf;

use super::constants::*;
use super::helpers::parse_env;
use crate::service::MemoryError;

/// Exact selector for `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`.
pub const SELECTOR_SAUKRAUT_LFM25: &str = "VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER";

/// Exact selector for classic DeBERTa GLiNER `urchade/gliner_multi-v2.1`.
pub const SELECTOR_CLASSIC_GLINER: &str = "urchade/gliner_multi-v2.1";

/// Closed catalog of supported NER extractor kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NerExtractorKind {
    /// Lightweight dependency-free Anno rule-based extraction (zero-config default).
    Anno,
    /// Project-owned deterministic regex extractor.
    Regex,
    /// Anno NuNER ONNX local model backend (CPU-only).
    AnnoOnnx,
    /// Native Candle classic DeBERTa GLiNER.
    ClassicGliner,
    /// Native Candle LFM2 GLiNER (SauerkrautLM VAGO).
    SauerkrautLfm25,
}

/// Device backend for native Candle GLiNER variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlinerDeviceKind {
    /// CPU-only inference (default).
    Cpu,
    /// Apple Metal GPU; explicit requests fail when the backend cannot prepare it.
    Metal,
    /// Try Metal, fall back to CPU with an explicit diagnostic event.
    Auto,
}

/// Shared configuration controls for model-backed NER extractors.
#[derive(Debug, Clone)]
pub struct ModelBackedNerConfig {
    /// Optional override root for cached model artifacts.
    pub cache_dir: Option<PathBuf>,
    /// Normalized entity labels to extract.
    pub labels: Vec<String>,
    /// Explicit confidence threshold; each backend chooses its evaluated default otherwise.
    pub threshold: Option<f64>,
    /// Max concurrent inference operations.
    pub max_concurrency: usize,
    /// Seconds of inactivity before unloading a loaded model. Zero retains.
    pub idle_unload_secs: u64,
}

/// Native Candle GLiNER configuration shared by classic and LFM2 backends.
#[derive(Debug, Clone)]
pub struct NativeGlinerConfig {
    /// Shared model-backed controls.
    pub model: ModelBackedNerConfig,
    /// Batch size for inference.
    pub batch_size: usize,
    /// Max padded tokens per batch.
    pub max_batch_tokens: usize,
    /// Device backend selection.
    pub device: GlinerDeviceKind,
}

/// Typed extractor configuration. Each variant carries only the settings it
/// understands, so irrelevant overrides are rejected structurally.
#[derive(Debug, Clone)]
pub enum NerExtractorConfig {
    /// Lightweight Anno rules; no model controls.
    Anno,
    /// Regex heuristics; no model controls.
    Regex,
    /// Explicit Anno NuNER ONNX backend.
    AnnoOnnx(ModelBackedNerConfig),
    /// Classic DeBERTa GLiNER backend.
    ClassicGliner(NativeGlinerConfig),
    /// SauerkrautLM LFM2 GLiNER backend.
    SauerkrautLfm25(NativeGlinerConfig),
}

impl NerExtractorConfig {
    /// Returns the stable kind tag for dispatch.
    #[must_use]
    pub fn kind(&self) -> NerExtractorKind {
        match self {
            Self::Anno => NerExtractorKind::Anno,
            Self::Regex => NerExtractorKind::Regex,
            Self::AnnoOnnx(_) => NerExtractorKind::AnnoOnnx,
            Self::ClassicGliner(_) => NerExtractorKind::ClassicGliner,
            Self::SauerkrautLfm25(_) => NerExtractorKind::SauerkrautLfm25,
        }
    }
}

/// Top-level NER configuration.
#[derive(Debug, Clone)]
pub struct NerConfig {
    /// Selected extractor configuration.
    pub extractor: NerExtractorConfig,
}

impl Default for NerConfig {
    fn default() -> Self {
        Self {
            extractor: NerExtractorConfig::Anno,
        }
    }
}

/// Removed variables that must fail with migration guidance even when empty.
const REMOVED_NER_VARS: &[(&str, &str)] = &[
    ("NER_PROVIDER", "NER_EXTRACTOR"),
    ("NER_MODEL", "NER_EXTRACTOR"),
    ("NER_MODEL_DIR", "NER_CACHE_DIR"),
    ("NER_BATCH_SIZE", "GLINER_BATCH_SIZE"),
    ("NER_MAX_BATCH_TOKENS", "GLINER_MAX_BATCH_TOKENS"),
    ("NER_DEVICE", "GLINER_DEVICE"),
    ("GLINER_IDLE_UNLOAD_SECS", "NER_IDLE_UNLOAD_SECS"),
];

fn check_removed_variables() -> Result<(), MemoryError> {
    for (old, replacement) in REMOVED_NER_VARS {
        if env::var_os(old).is_some() {
            return Err(MemoryError::ConfigInvalid(format!(
                "{old} is no longer supported; use {replacement} instead (see ADR-0036)"
            )));
        }
    }
    Ok(())
}

fn normalize_labels(raw: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for label in raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
    {
        if seen.insert(label.clone()) {
            result.push(label);
        }
    }
    result
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

fn parse_nonzero_usize(var_name: &str, default: usize) -> Result<usize, MemoryError> {
    let value = parse_env::<usize>(var_name)?.unwrap_or(default);
    if value == 0 {
        return Err(MemoryError::ConfigInvalid(format!(
            "{var_name} must be greater than zero"
        )));
    }
    Ok(value)
}

/// Fuzzy-match threshold for entity alias resolution (0.0..=1.0).
///
/// Reads `ENTITY_FUZZY_THRESHOLD`; invalid values are rejected with
/// [`MemoryError::ConfigInvalid`] instead of silently falling back.
pub fn entity_fuzzy_threshold() -> Result<f64, MemoryError> {
    let value = parse_env::<f64>("ENTITY_FUZZY_THRESHOLD")?
        .unwrap_or(crate::service::entity_resolution::DEFAULT_FUZZY_THRESHOLD);
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(MemoryError::ConfigInvalid(
            "ENTITY_FUZZY_THRESHOLD must be a finite number in 0.0..=1.0".to_string(),
        ));
    }
    Ok(value)
}

fn parse_threshold(var_name: &str) -> Result<Option<f64>, MemoryError> {
    let Some(value) = parse_env::<f64>(var_name)? else {
        return Ok(None);
    };
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(MemoryError::ConfigInvalid(format!(
            "{var_name} must be a finite number in 0.0..=1.0"
        )));
    }
    Ok(Some(value))
}

fn parse_gliner_device(var_name: &str) -> Result<GlinerDeviceKind, MemoryError> {
    match env::var(var_name)
        .unwrap_or_else(|_| "cpu".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "cpu" => Ok(GlinerDeviceKind::Cpu),
        "metal" => Ok(GlinerDeviceKind::Metal),
        "auto" => Ok(GlinerDeviceKind::Auto),
        other => Err(MemoryError::ConfigInvalid(format!(
            "unsupported {var_name} `{other}`; expected cpu, metal, or auto"
        ))),
    }
}

fn model_backed_config() -> Result<ModelBackedNerConfig, MemoryError> {
    let labels = env::var("NER_LABELS")
        .map(|raw| normalize_labels(&raw))
        .unwrap_or_else(|_| default_ner_labels());
    Ok(ModelBackedNerConfig {
        cache_dir: env::var("NER_CACHE_DIR").ok().map(PathBuf::from),
        labels,
        threshold: parse_threshold("NER_THRESHOLD")?,
        max_concurrency: parse_nonzero_usize("NER_MAX_CONCURRENCY", DEFAULT_NER_MAX_CONCURRENCY)?,
        idle_unload_secs: parse_env::<u64>("NER_IDLE_UNLOAD_SECS")?
            .unwrap_or(DEFAULT_NER_IDLE_UNLOAD_SECS),
    })
}

fn native_gliner_config() -> Result<NativeGlinerConfig, MemoryError> {
    Ok(NativeGlinerConfig {
        model: model_backed_config()?,
        batch_size: parse_nonzero_usize("GLINER_BATCH_SIZE", DEFAULT_GLINER_BATCH_SIZE)?,
        max_batch_tokens: parse_nonzero_usize(
            "GLINER_MAX_BATCH_TOKENS",
            DEFAULT_GLINER_MAX_BATCH_TOKENS,
        )?,
        device: parse_gliner_device("GLINER_DEVICE")?,
    })
}

/// Variables accepted by lightweight Anno and Regex selectors.
const LIGHTWEIGHT_ALLOWED_PREFIXES: &[&str] = &[];
/// Model-backed shared controls.
const MODEL_BACKED_VARS: &[&str] = &[
    "NER_CACHE_DIR",
    "NER_LABELS",
    "NER_THRESHOLD",
    "NER_MAX_CONCURRENCY",
    "NER_IDLE_UNLOAD_SECS",
];
/// Native GLiNER-only controls.
const NATIVE_GLINER_VARS: &[&str] = &[
    "GLINER_BATCH_SIZE",
    "GLINER_MAX_BATCH_TOKENS",
    "GLINER_DEVICE",
];

fn reject_irrelevant_settings(kind: NerExtractorKind) -> Result<(), MemoryError> {
    match kind {
        NerExtractorKind::Anno | NerExtractorKind::Regex => {
            for var in MODEL_BACKED_VARS.iter().chain(NATIVE_GLINER_VARS) {
                if env::var_os(var).is_some() {
                    return Err(MemoryError::ConfigInvalid(format!(
                        "{var} is irrelevant when NER_EXTRACTOR selects a lightweight extractor"
                    )));
                }
            }
        }
        NerExtractorKind::AnnoOnnx => {
            for var in NATIVE_GLINER_VARS {
                if env::var_os(var).is_some() {
                    return Err(MemoryError::ConfigInvalid(format!(
                        "{var} is irrelevant when NER_EXTRACTOR=`anno-onnx`"
                    )));
                }
            }
        }
        NerExtractorKind::ClassicGliner | NerExtractorKind::SauerkrautLfm25 => {}
    }
    let _ = LIGHTWEIGHT_ALLOWED_PREFIXES;
    Ok(())
}

impl NerConfig {
    /// Loads NER extractor configuration from the environment.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::ConfigInvalid`] on removed variables, unknown
    /// selectors, irrelevant settings, malformed numerics, or out-of-range
    /// threshold.
    pub fn from_env() -> Result<Self, MemoryError> {
        check_removed_variables()?;

        let selector = env::var("NER_EXTRACTOR")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| "anno".to_string());

        let extractor = match selector.as_str() {
            "anno" => NerExtractorConfig::Anno,
            "regex" => NerExtractorConfig::Regex,
            "anno-onnx" => NerExtractorConfig::AnnoOnnx(model_backed_config()?),
            SELECTOR_CLASSIC_GLINER => NerExtractorConfig::ClassicGliner(native_gliner_config()?),
            SELECTOR_SAUKRAUT_LFM25 => NerExtractorConfig::SauerkrautLfm25(native_gliner_config()?),
            other => {
                return Err(MemoryError::ConfigInvalid(format!(
                    "unknown NER_EXTRACTOR `{other}`; expected one of: anno, regex, anno-onnx, {SELECTOR_CLASSIC_GLINER}, {SELECTOR_SAUKRAUT_LFM25}"
                )));
            }
        };

        reject_irrelevant_settings(extractor.kind())?;

        Ok(Self { extractor })
    }
}

#[cfg(test)]
mod tests {
    use super::super::env_lock;
    use super::*;

    const NER_ENV_KEYS: &[&str] = &[
        "NER_EXTRACTOR",
        "NER_CACHE_DIR",
        "NER_LABELS",
        "NER_THRESHOLD",
        "NER_MAX_CONCURRENCY",
        "NER_IDLE_UNLOAD_SECS",
        "GLINER_BATCH_SIZE",
        "GLINER_MAX_BATCH_TOKENS",
        "GLINER_DEVICE",
        "NER_PROVIDER",
        "NER_MODEL",
        "NER_MODEL_DIR",
        "NER_BATCH_SIZE",
        "NER_MAX_BATCH_TOKENS",
        "NER_DEVICE",
        "GLINER_IDLE_UNLOAD_SECS",
    ];

    fn with_ner_env(vars: &[(&str, Option<&str>)], test: impl FnOnce()) {
        let _guard = env_lock().lock().expect("NER env lock");
        let saved: Vec<(String, Option<String>)> = NER_ENV_KEYS
            .iter()
            .map(|key| ((*key).to_string(), std::env::var(key).ok()))
            .collect();
        for key in NER_ENV_KEYS {
            unsafe { std::env::remove_var(key) };
        }
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
                    Some(value) => std::env::set_var(&key, value),
                    None => std::env::remove_var(&key),
                }
            }
        }
        outcome.expect("NER config test body");
    }

    #[test]
    fn empty_env_selects_lightweight_anno() {
        with_ner_env(&[], || {
            let config = NerConfig::from_env().expect("default config");
            assert!(matches!(config.extractor, NerExtractorConfig::Anno));
        });
    }

    #[test]
    fn extractor_catalog_parses_exact_selectors() {
        for (selector, expected_kind) in [
            ("anno", NerExtractorKind::Anno),
            ("regex", NerExtractorKind::Regex),
            ("anno-onnx", NerExtractorKind::AnnoOnnx),
            (SELECTOR_CLASSIC_GLINER, NerExtractorKind::ClassicGliner),
            (SELECTOR_SAUKRAUT_LFM25, NerExtractorKind::SauerkrautLfm25),
        ] {
            with_ner_env(&[("NER_EXTRACTOR", Some(selector))], || {
                let config = NerConfig::from_env().expect("typed config");
                assert_eq!(config.extractor.kind(), expected_kind);
            });
        }
    }

    #[test]
    fn unknown_selector_is_rejected() {
        with_ner_env(&[("NER_EXTRACTOR", Some("other/model"))], || {
            let err = NerConfig::from_env().expect_err("unknown selector must fail");
            assert!(matches!(err, MemoryError::ConfigInvalid(_)));
        });
    }

    #[test]
    fn removed_variables_fail_with_migration_guidance() {
        for (old, replacement) in REMOVED_NER_VARS {
            with_ner_env(
                &[(old, Some("legacy")), ("NER_EXTRACTOR", Some("regex"))],
                || {
                    let err = NerConfig::from_env().expect_err("removed variable must fail");
                    let message = match err {
                        MemoryError::ConfigInvalid(message) => message,
                        other => panic!("expected ConfigInvalid, got {other:?}"),
                    };
                    assert!(message.contains(old), "message must name {old}: {message}");
                    assert!(
                        message.contains(replacement),
                        "message must suggest {replacement}: {message}"
                    );
                },
            );
        }
    }

    #[test]
    fn removed_variable_fails_even_when_empty() {
        with_ner_env(&[("NER_PROVIDER", Some(""))], || {
            assert!(matches!(
                NerConfig::from_env(),
                Err(MemoryError::ConfigInvalid(_))
            ));
        });
    }

    #[test]
    fn gliner_settings_rejected_for_lightweight_extractors() {
        for selector in ["anno", "regex"] {
            with_ner_env(
                &[
                    ("NER_EXTRACTOR", Some(selector)),
                    ("GLINER_DEVICE", Some("metal")),
                ],
                || {
                    let err = NerConfig::from_env()
                        .expect_err("gliner settings must be rejected for anno/regex");
                    assert!(matches!(err, MemoryError::ConfigInvalid(_)));
                },
            );
        }
    }

    #[test]
    fn gliner_settings_rejected_for_anno_onnx() {
        with_ner_env(
            &[
                ("NER_EXTRACTOR", Some("anno-onnx")),
                ("GLINER_BATCH_SIZE", Some("4")),
            ],
            || {
                let err =
                    NerConfig::from_env().expect_err("gliner batch size irrelevant for anno-onnx");
                assert!(matches!(err, MemoryError::ConfigInvalid(_)));
            },
        );
    }

    #[test]
    fn labels_are_normalized_deduplicated_and_lowercased() {
        with_ner_env(
            &[
                ("NER_EXTRACTOR", Some("anno-onnx")),
                ("NER_LABELS", Some(" Person,COMPANY, person, ,Location ")),
            ],
            || {
                let config = NerConfig::from_env().expect("labels parse");
                let NerExtractorConfig::AnnoOnnx(cfg) = config.extractor else {
                    panic!("expected AnnoOnnx");
                };
                assert_eq!(cfg.labels, vec!["person", "company", "location"]);
            },
        );
    }

    #[test]
    fn threshold_must_be_finite_within_unit_interval() {
        for raw in ["-0.1", "1.01", "NaN", "inf"] {
            with_ner_env(
                &[
                    ("NER_EXTRACTOR", Some("anno-onnx")),
                    ("NER_THRESHOLD", Some(raw)),
                ],
                || {
                    assert!(matches!(
                        NerConfig::from_env(),
                        Err(MemoryError::ConfigInvalid(_))
                    ));
                },
            );
        }
    }

    #[test]
    fn threshold_is_optional_and_in_range_values_pass() {
        with_ner_env(
            &[
                ("NER_EXTRACTOR", Some(SELECTOR_CLASSIC_GLINER)),
                ("NER_THRESHOLD", Some("0.3")),
            ],
            || {
                let config = NerConfig::from_env().expect("threshold parses");
                let NerExtractorConfig::ClassicGliner(cfg) = config.extractor else {
                    panic!("expected ClassicGliner");
                };
                assert_eq!(cfg.model.threshold, Some(0.3));
            },
        );
    }

    #[test]
    fn nonzero_limits_are_enforced() {
        for var in [
            "NER_MAX_CONCURRENCY",
            "GLINER_BATCH_SIZE",
            "GLINER_MAX_BATCH_TOKENS",
        ] {
            with_ner_env(
                &[
                    ("NER_EXTRACTOR", Some(SELECTOR_CLASSIC_GLINER)),
                    (var, Some("0")),
                ],
                || {
                    assert!(matches!(
                        NerConfig::from_env(),
                        Err(MemoryError::ConfigInvalid(_))
                    ));
                },
            );
        }
    }

    #[test]
    fn idle_unload_defaults_to_zero_and_reads_override() {
        with_ner_env(&[("NER_EXTRACTOR", Some("anno-onnx"))], || {
            let config = NerConfig::from_env().expect("default idle");
            let NerExtractorConfig::AnnoOnnx(cfg) = config.extractor else {
                panic!("expected AnnoOnnx");
            };
            assert_eq!(cfg.idle_unload_secs, 0);
        });
        with_ner_env(
            &[
                ("NER_EXTRACTOR", Some("anno-onnx")),
                ("NER_IDLE_UNLOAD_SECS", Some("60")),
            ],
            || {
                let config = NerConfig::from_env().expect("override idle");
                let NerExtractorConfig::AnnoOnnx(cfg) = config.extractor else {
                    panic!("expected AnnoOnnx");
                };
                assert_eq!(cfg.idle_unload_secs, 60);
            },
        );
    }

    #[test]
    fn native_gliner_device_defaults_to_cpu_and_accepts_metal_auto() {
        with_ner_env(&[("NER_EXTRACTOR", Some(SELECTOR_CLASSIC_GLINER))], || {
            let config = NerConfig::from_env().expect("default device");
            let NerExtractorConfig::ClassicGliner(cfg) = config.extractor else {
                panic!("expected ClassicGliner");
            };
            assert_eq!(cfg.device, GlinerDeviceKind::Cpu);
        });
        for (raw, expected) in [
            ("metal", GlinerDeviceKind::Metal),
            ("auto", GlinerDeviceKind::Auto),
        ] {
            with_ner_env(
                &[
                    ("NER_EXTRACTOR", Some(SELECTOR_SAUKRAUT_LFM25)),
                    ("GLINER_DEVICE", Some(raw)),
                ],
                || {
                    let config = NerConfig::from_env().expect("explicit device");
                    let NerExtractorConfig::SauerkrautLfm25(cfg) = config.extractor else {
                        panic!("expected SauerkrautLfm25");
                    };
                    assert_eq!(cfg.device, expected);
                },
            );
        }
    }

    #[test]
    fn native_gliner_device_rejects_unknown_backend() {
        with_ner_env(
            &[
                ("NER_EXTRACTOR", Some(SELECTOR_CLASSIC_GLINER)),
                ("GLINER_DEVICE", Some("coreml")),
            ],
            || {
                assert!(matches!(
                    NerConfig::from_env(),
                    Err(MemoryError::ConfigInvalid(_))
                ));
            },
        );
    }

    #[test]
    fn cache_dir_is_accepted_for_model_backed_extractors() {
        with_ner_env(
            &[
                ("NER_EXTRACTOR", Some("anno-onnx")),
                ("NER_CACHE_DIR", Some("/tmp/ner-cache")),
            ],
            || {
                let config = NerConfig::from_env().expect("cache dir parses");
                let NerExtractorConfig::AnnoOnnx(cfg) = config.extractor else {
                    panic!("expected AnnoOnnx");
                };
                assert_eq!(cfg.cache_dir, Some(PathBuf::from("/tmp/ner-cache")));
            },
        );
    }
}
