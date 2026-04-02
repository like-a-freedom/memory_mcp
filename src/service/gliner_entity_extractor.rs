//! GLiNER-based NER extractor using Candle inference engine.
//!
//! This module provides zero-shot NER extraction using GLiNER models.
//! Requires model weights from HuggingFace (e.g., `urchade/gliner_multi-v2.1`).

use std::{collections::HashMap, path::Path};

use async_trait::async_trait;
use candle_core::{Device, IndexOp, Module, Tensor};
use candle_nn::rnn::Direction;
use candle_nn::{LSTM, LSTMConfig, RNN, VarBuilder};
use candle_transformers::models::debertav2::{Config, DTYPE, DebertaV2Model};
use tokenizers::Tokenizer;

use super::EntityExtractor;
use crate::models::EntityCandidate;
use crate::service::MemoryError;

const ENT_TOKEN_CANDIDATES: &[&str] = &["<<ENT>>", "[ENT]", "<<SEP>>", "@"];
const DEFAULT_MAX_SPAN_WIDTH: usize = 12;
const DEFAULT_MAX_SEQ_LEN: usize = 384;
const FALLBACK_BACKBONE_MAX_POSITION_EMBEDDINGS: usize = 512;
const BACKBONE_PREFIX: &str = "token_rep_layer.bert_layer.model";

#[derive(Debug, Clone)]
struct PromptEncoding {
    token_ids: Vec<u32>,
    entity_token_positions: Vec<usize>,
}

#[derive(Debug, Clone)]
struct ScoredSpan {
    start: usize,
    end: usize,
    text: String,
    label: String,
    score: f32,
}

#[derive(Clone)]
pub struct GlinerEntityExtractor {
    model: std::sync::Arc<DebertaV2Model>,
    tokenizer: std::sync::Arc<Tokenizer>,
    device: Device,
    labels: Vec<String>,
    threshold: f64,
    max_span_width: usize,
    max_seq_len: usize,
    ent_token_id: u32,
    sep_token_id: u32,
    token_projection: std::sync::Arc<TokenProjectionLayer>,
    rnn: std::sync::Arc<BiLstmLayer>,
    span_rep_layer: std::sync::Arc<SpanRepresentationLayer>,
    prompt_rep_layer: std::sync::Arc<FeedForwardProjection>,
}

#[derive(Debug, Clone)]
struct GlinerRuntimeConfig {
    backbone: Config,
    head_hidden_size: usize,
    max_span_width: usize,
    max_seq_len: usize,
}

/// GLiNER config JSON has different fields than DeBERTa config.
/// This struct captures the GLiNER-specific fields for mapping.
#[derive(serde::Deserialize)]
struct GlinerConfig {
    #[serde(default = "default_hidden_size")]
    hidden_size: usize,
    #[serde(default = "default_max_position", rename = "max_len")]
    max_position_embeddings: usize,
    #[serde(default = "default_dropout", rename = "dropout")]
    hidden_dropout_prob: f64,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default = "default_max_span_width", rename = "max_width")]
    max_span_width: usize,
}

fn default_hidden_size() -> usize {
    512
}
fn default_max_position() -> usize {
    384
}
fn default_dropout() -> f64 {
    0.1
}

fn default_max_span_width() -> usize {
    DEFAULT_MAX_SPAN_WIDTH
}

fn resolve_sep_token(tokenizer: &Tokenizer) -> Result<u32, MemoryError> {
    tokenizer.token_to_id("<<SEP>>").ok_or_else(|| {
        MemoryError::Storage("GLiNER tokenizer missing separator token `<<SEP>>`".to_string())
    })
}

fn gliner_ffn_hidden_size(hidden_size: usize) -> usize {
    hidden_size.saturating_mul(4)
}

#[derive(Debug, serde::Deserialize)]
struct SafetensorsTensorMetadata {
    shape: Vec<usize>,
}

#[derive(Debug)]
struct TokenProjectionLayer {
    projection: Option<candle_nn::Linear>,
}

impl TokenProjectionLayer {
    fn load(vb: VarBuilder, input_dim: usize, output_dim: usize) -> candle_core::Result<Self> {
        // When dimensions match, projection may be absent in some model weights (identity).
        // When present, load it; propagate real errors instead of silently swallowing them.
        let projection = if input_dim == output_dim {
            match candle_nn::linear(input_dim, output_dim, vb.pp("projection")) {
                Ok(linear) => Some(linear),
                Err(candle_core::Error::CannotFindTensor { .. }) => None,
                Err(e) => return Err(e),
            }
        } else {
            Some(candle_nn::linear(
                input_dim,
                output_dim,
                vb.pp("projection"),
            )?)
        };

        Ok(Self { projection })
    }

    fn forward(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        match &self.projection {
            Some(projection) => projection.forward(xs),
            None => Ok(xs.clone()),
        }
    }
}

#[derive(Debug)]
struct BiLstmLayer {
    forward: LSTM,
    backward: LSTM,
}

impl BiLstmLayer {
    fn load(vb: VarBuilder, input_dim: usize, hidden_dim: usize) -> candle_core::Result<Self> {
        if hidden_dim == 0 || !hidden_dim.is_multiple_of(2) {
            return Err(candle_core::Error::Msg(
                "GLiNER rnn hidden size must be a positive even number".to_string(),
            ));
        }

        let forward = candle_nn::lstm(
            input_dim,
            hidden_dim,
            LSTMConfig {
                direction: Direction::Forward,
                ..Default::default()
            },
            vb.pp("lstm"),
        )?;
        let backward = candle_nn::lstm(
            input_dim,
            hidden_dim,
            LSTMConfig {
                direction: Direction::Backward,
                ..Default::default()
            },
            vb.pp("lstm"),
        )?;

        Ok(Self { forward, backward })
    }

    fn reverse_time_axis(xs: &Tensor) -> candle_core::Result<Tensor> {
        let seq_len = xs.dim(1)?;
        let mut steps = Vec::with_capacity(seq_len);
        for idx in (0..seq_len).rev() {
            steps.push(xs.i((.., idx, ..))?.contiguous()?);
        }

        let refs = steps.iter().collect::<Vec<_>>();
        Tensor::stack(&refs, 1)
    }

    fn forward(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        let xs = xs.unsqueeze(0)?;

        let forward_states = self.forward.seq(&xs)?;
        let forward_hidden = forward_states
            .into_iter()
            .map(|state| state.h)
            .collect::<Vec<_>>();
        let forward_refs = forward_hidden.iter().collect::<Vec<_>>();
        let forward = Tensor::stack(&forward_refs, 1)?;

        let reversed_xs = Self::reverse_time_axis(&xs)?;
        let backward_states = self.backward.seq(&reversed_xs)?;
        let mut backward_hidden = backward_states
            .into_iter()
            .map(|state| state.h)
            .collect::<Vec<_>>();
        backward_hidden.reverse();
        let backward_refs = backward_hidden.iter().collect::<Vec<_>>();
        let backward = Tensor::stack(&backward_refs, 1)?;

        Tensor::cat(&[&forward, &backward], 2)?.squeeze(0)
    }
}

#[derive(Debug)]
struct FeedForwardProjection {
    input: candle_nn::Linear,
    output: candle_nn::Linear,
}

impl FeedForwardProjection {
    fn load(
        vb: VarBuilder,
        input_dim: usize,
        hidden_dim: usize,
        output_dim: usize,
    ) -> candle_core::Result<Self> {
        let input = candle_nn::linear(input_dim, hidden_dim, vb.pp("0"))?;
        let output = candle_nn::linear(hidden_dim, output_dim, vb.pp("3"))?;
        Ok(Self { input, output })
    }

    fn forward(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        let hidden = self.input.forward(xs)?.relu()?;
        self.output.forward(&hidden)
    }
}

#[derive(Debug)]
struct SpanRepresentationLayer {
    project_start: FeedForwardProjection,
    project_end: FeedForwardProjection,
    out_project: FeedForwardProjection,
}

impl SpanRepresentationLayer {
    fn load(vb: VarBuilder, hidden_size: usize) -> candle_core::Result<Self> {
        let intermediate = gliner_ffn_hidden_size(hidden_size);
        // The caller passes the exact weight prefix required by the model layout.
        // This loader only appends the projection-specific segments.
        let project_start = FeedForwardProjection::load(
            vb.pp("project_start"),
            hidden_size,
            intermediate,
            hidden_size,
        )?;
        let project_end = FeedForwardProjection::load(
            vb.pp("project_end"),
            hidden_size,
            intermediate,
            hidden_size,
        )?;
        let out_project = FeedForwardProjection::load(
            vb.pp("out_project"),
            hidden_size * 2,
            intermediate,
            hidden_size,
        )?;
        Ok(Self {
            project_start,
            project_end,
            out_project,
        })
    }

    fn forward(&self, start_hidden: &Tensor, end_hidden: &Tensor) -> candle_core::Result<Tensor> {
        let start = self.project_start.forward(start_hidden)?;
        let end = self.project_end.forward(end_hidden)?;
        let combined = Tensor::cat(&[&start, &end], 1)?;
        self.out_project.forward(&combined)
    }
}

fn parse_gliner_runtime_config(
    json_str: &str,
    safetensors_path: Option<&Path>,
) -> Result<GlinerRuntimeConfig, MemoryError> {
    let gc: GlinerConfig = serde_json::from_str(json_str)
        .map_err(|e| MemoryError::Storage(format!("failed to parse GLiNER config: {e}")))?;
    let backbone = match safetensors_path {
        Some(path) => {
            let metadata = read_safetensors_metadata(path)?;
            infer_backbone_config_from_metadata(
                &metadata,
                gc.max_position_embeddings,
                gc.hidden_dropout_prob,
            )?
        }
        None => infer_backbone_config_from_model_name(
            gc.model_name.as_deref(),
            gc.max_position_embeddings,
            gc.hidden_dropout_prob,
        )?,
    };

    Ok(GlinerRuntimeConfig {
        backbone,
        head_hidden_size: gc.hidden_size,
        max_span_width: gc.max_span_width,
        max_seq_len: gc.max_position_embeddings.max(DEFAULT_MAX_SEQ_LEN),
    })
}

fn read_safetensors_metadata(
    path: &Path,
) -> Result<HashMap<String, SafetensorsTensorMetadata>, MemoryError> {
    let bytes = std::fs::read(path)
        .map_err(|e| MemoryError::Storage(format!("failed to read safetensors header: {e}")))?;
    if bytes.len() < 8 {
        return Err(MemoryError::Storage(
            "safetensors file is too short to contain a header".to_string(),
        ));
    }

    let header_len_bytes: [u8; 8] = bytes[..8].try_into().map_err(|_| {
        MemoryError::Storage("failed to decode safetensors header length".to_string())
    })?;
    let header_len = u64::from_le_bytes(header_len_bytes) as usize;
    let header_start = 8;
    let header_end = header_start + header_len;
    if header_end > bytes.len() {
        return Err(MemoryError::Storage(
            "safetensors header length exceeds file length".to_string(),
        ));
    }

    serde_json::from_slice(&bytes[header_start..header_end]).map_err(|e| {
        MemoryError::Storage(format!("failed to parse safetensors header metadata: {e}"))
    })
}

fn infer_backbone_config_from_metadata(
    metadata: &HashMap<String, SafetensorsTensorMetadata>,
    max_seq_len: usize,
    hidden_dropout_prob: f64,
) -> Result<Config, MemoryError> {
    let word_embeddings = metadata
        .get(&format!("{BACKBONE_PREFIX}.embeddings.word_embeddings.weight"))
        .ok_or_else(|| {
            MemoryError::Storage(
                "GLiNER weights are missing token_rep_layer.bert_layer.model.embeddings.word_embeddings.weight"
                    .to_string(),
            )
        })?;
    let [vocab_size, hidden_size] = word_embeddings.shape.as_slice() else {
        return Err(MemoryError::Storage(
            "GLiNER word embeddings must be rank-2".to_string(),
        ));
    };

    let intermediate_weight_key =
        format!("{BACKBONE_PREFIX}.encoder.layer.0.intermediate.dense.weight");
    let intermediate_weight = metadata.get(&intermediate_weight_key).ok_or_else(|| {
        MemoryError::Storage(format!(
            "GLiNER weights are missing {intermediate_weight_key}"
        ))
    })?;
    let [intermediate_size, _] = intermediate_weight.shape.as_slice() else {
        return Err(MemoryError::Storage(
            "GLiNER intermediate dense weight must be rank-2".to_string(),
        ));
    };

    let num_hidden_layers = metadata
        .keys()
        .filter_map(|key| {
            key.strip_prefix(&format!("{BACKBONE_PREFIX}.encoder.layer."))
                .and_then(|suffix| suffix.split('.').next())
                .and_then(|index| index.parse::<usize>().ok())
        })
        .max()
        .map(|max_index| max_index + 1)
        .ok_or_else(|| {
            MemoryError::Storage(
                "GLiNER weights do not contain any DeBERTa encoder layers".to_string(),
            )
        })?;

    let num_attention_heads = if hidden_size % 64 == 0 {
        hidden_size / 64
    } else {
        return Err(MemoryError::Storage(format!(
            "cannot infer DeBERTa attention head count from hidden size {hidden_size}"
        )));
    };

    let position_embeddings_key =
        format!("{BACKBONE_PREFIX}.embeddings.position_embeddings.weight");
    let token_type_embeddings_key =
        format!("{BACKBONE_PREFIX}.embeddings.token_type_embeddings.weight");
    let rel_embeddings_key = format!("{BACKBONE_PREFIX}.encoder.rel_embeddings.weight");
    let encoder_layer_norm_key = format!("{BACKBONE_PREFIX}.encoder.LayerNorm.weight");

    let position_biased_input = metadata.contains_key(&position_embeddings_key);
    let type_vocab_size = metadata
        .get(&token_type_embeddings_key)
        .and_then(|entry| entry.shape.first().copied())
        .unwrap_or(0);
    let position_buckets = metadata
        .get(&rel_embeddings_key)
        .and_then(|entry| entry.shape.first().copied())
        .map(|size| (size / 2) as isize);

    Ok(Config {
        vocab_size: *vocab_size,
        hidden_size: *hidden_size,
        num_hidden_layers,
        num_attention_heads,
        intermediate_size: *intermediate_size,
        hidden_act: candle_transformers::models::debertav2::HiddenAct::Gelu,
        hidden_dropout_prob,
        attention_probs_dropout_prob: hidden_dropout_prob,
        max_position_embeddings: if position_biased_input {
            metadata
                .get(&position_embeddings_key)
                .and_then(|entry| entry.shape.first().copied())
                .unwrap_or(FALLBACK_BACKBONE_MAX_POSITION_EMBEDDINGS)
        } else {
            max_seq_len.max(FALLBACK_BACKBONE_MAX_POSITION_EMBEDDINGS)
        },
        type_vocab_size,
        initializer_range: 0.02,
        layer_norm_eps: 1e-7,
        relative_attention: metadata.contains_key(&rel_embeddings_key),
        max_relative_positions: -1,
        pad_token_id: Some(0),
        position_biased_input,
        pos_att_type: vec!["p2c".to_string(), "c2p".to_string()],
        position_buckets,
        share_att_key: Some(true),
        attention_head_size: None,
        embedding_size: None,
        norm_rel_ebd: metadata
            .contains_key(&encoder_layer_norm_key)
            .then(|| "layer_norm".to_string()),
        conv_kernel_size: None,
        conv_groups: None,
        conv_act: None,
        id2label: None,
        label2id: None,
        pooler_dropout: None,
        pooler_hidden_act: None,
        pooler_hidden_size: None,
        cls_dropout: None,
    })
}

fn infer_backbone_config_from_model_name(
    model_name: Option<&str>,
    max_seq_len: usize,
    hidden_dropout_prob: f64,
) -> Result<Config, MemoryError> {
    let normalized = model_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("microsoft/mdeberta-v3-base")
        .to_ascii_lowercase();

    match normalized.as_str() {
        "microsoft/mdeberta-v3-base" | "mdeberta-v3-base" | "deberta-v3-base" => Ok(Config {
            vocab_size: 250_105,
            hidden_size: 768,
            num_hidden_layers: 12,
            num_attention_heads: 12,
            intermediate_size: 3072,
            hidden_act: candle_transformers::models::debertav2::HiddenAct::Gelu,
            hidden_dropout_prob,
            attention_probs_dropout_prob: hidden_dropout_prob,
            max_position_embeddings: max_seq_len.max(FALLBACK_BACKBONE_MAX_POSITION_EMBEDDINGS),
            type_vocab_size: 0,
            initializer_range: 0.02,
            layer_norm_eps: 1e-7,
            relative_attention: true,
            max_relative_positions: -1,
            pad_token_id: Some(0),
            position_biased_input: false,
            pos_att_type: vec!["p2c".to_string(), "c2p".to_string()],
            position_buckets: Some(256),
            share_att_key: Some(true),
            attention_head_size: None,
            embedding_size: None,
            norm_rel_ebd: Some("layer_norm".to_string()),
            conv_kernel_size: None,
            conv_groups: None,
            conv_act: None,
            id2label: None,
            label2id: None,
            pooler_dropout: None,
            pooler_hidden_act: None,
            pooler_hidden_size: None,
            cls_dropout: None,
        }),
        other => Err(MemoryError::Storage(format!(
            "unsupported GLiNER backbone model_name `{other}` without safetensors metadata"
        ))),
    }
}

impl GlinerEntityExtractor {
    pub fn new(model_dir: &Path, labels: Vec<String>, threshold: f64) -> Result<Self, MemoryError> {
        let tokenizer_path = model_dir.join("tokenizer.json");

        // GLiNER models use gliner_config.json, standard models use config.json
        let config_path = if model_dir.join("gliner_config.json").exists() {
            model_dir.join("gliner_config.json")
        } else {
            model_dir.join("config.json")
        };

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| MemoryError::Storage(format!("failed to load tokenizer: {}", e)))?;

        let config_str = std::fs::read_to_string(&config_path)
            .map_err(|e| MemoryError::Storage(format!("failed to read config: {}", e)))?;

        // Prefer model.safetensors (updated Dec 2025 with full weights)
        let safetensors_path = model_dir.join("model.safetensors");
        let pytorch_path = model_dir.join("pytorch_model.bin");

        // Parse config - order depends on file type
        // For gliner_config.json, use GLiNER-specific parsing first
        // For standard config.json, use DeBERTa parsing first
        let runtime_config = if config_path
            .file_name()
            .map(|n| n == "gliner_config.json")
            .unwrap_or(false)
        {
            parse_gliner_runtime_config(
                &config_str,
                safetensors_path
                    .is_file()
                    .then_some(safetensors_path.as_path()),
            )
            .map_err(|e| MemoryError::Storage(format!("failed to parse config: {e}")))?
        } else {
            let backbone: Config = serde_json::from_str(&config_str)
                .map_err(|e| MemoryError::Storage(format!("failed to parse config: {}", e)))?;
            GlinerRuntimeConfig {
                head_hidden_size: backbone.hidden_size,
                max_span_width: DEFAULT_MAX_SPAN_WIDTH,
                max_seq_len: backbone.max_position_embeddings.max(DEFAULT_MAX_SEQ_LEN),
                backbone,
            }
        };

        let device = Device::Cpu;

        if safetensors_path.is_file() {
            // Load from safetensors
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&safetensors_path], DTYPE, &device)
            }
            .map_err(|e| MemoryError::Storage(format!("failed to load safetensors: {}", e)))?;

            let ent_token_id = Self::resolve_ent_token(&tokenizer)?;
            let sep_token_id = resolve_sep_token(&tokenizer)?;

            let model = DebertaV2Model::load(vb.pp(BACKBONE_PREFIX), &runtime_config.backbone)
                .map_err(|e| MemoryError::Storage(format!("failed to build model: {}", e)))?;

            let token_projection = TokenProjectionLayer::load(
                vb.pp("token_rep_layer"),
                runtime_config.backbone.hidden_size,
                runtime_config.head_hidden_size,
            )
            .map_err(|e| MemoryError::Storage(format!("failed to load token projection: {}", e)))?;
            let rnn = BiLstmLayer::load(
                vb.pp("rnn"),
                runtime_config.head_hidden_size,
                runtime_config.head_hidden_size / 2,
            )
            .map_err(|e| MemoryError::Storage(format!("failed to load rnn: {}", e)))?;
            // Real weights use double prefix: span_rep_layer.span_rep_layer.*
            let span_rep_layer = SpanRepresentationLayer::load(
                vb.pp("span_rep_layer").pp("span_rep_layer"),
                runtime_config.head_hidden_size,
            )
            .map_err(|e| MemoryError::Storage(format!("failed to load span_rep_layer: {}", e)))?;
            let prompt_hidden = gliner_ffn_hidden_size(runtime_config.head_hidden_size);
            let prompt_rep_layer = FeedForwardProjection::load(
                vb.pp("prompt_rep_layer"),
                runtime_config.head_hidden_size,
                prompt_hidden,
                runtime_config.head_hidden_size,
            )
            .map_err(|e| MemoryError::Storage(format!("failed to load prompt_rep_layer: {}", e)))?;

            Ok(Self {
                model: std::sync::Arc::new(model),
                tokenizer: std::sync::Arc::new(tokenizer),
                device,
                labels,
                threshold,
                max_span_width: runtime_config.max_span_width,
                max_seq_len: runtime_config.max_seq_len,
                ent_token_id,
                sep_token_id,
                token_projection: std::sync::Arc::new(token_projection),
                rnn: std::sync::Arc::new(rnn),
                span_rep_layer: std::sync::Arc::new(span_rep_layer),
                prompt_rep_layer: std::sync::Arc::new(prompt_rep_layer),
            })
        } else if pytorch_path.is_file() {
            // Fallback to pytorch_model.bin
            let ent_token_id = Self::resolve_ent_token(&tokenizer)?;
            let sep_token_id = resolve_sep_token(&tokenizer)?;

            let vb = VarBuilder::from_pth(pytorch_path.to_str().unwrap_or(""), DTYPE, &device)
                .map_err(|e| {
                    MemoryError::Storage(format!("failed to load pytorch weights: {}", e))
                })?;

            let model = DebertaV2Model::load(vb.pp(BACKBONE_PREFIX), &runtime_config.backbone)
                .map_err(|e| MemoryError::Storage(format!("failed to build model: {}", e)))?;

            let token_projection = TokenProjectionLayer::load(
                vb.pp("token_rep_layer"),
                runtime_config.backbone.hidden_size,
                runtime_config.head_hidden_size,
            )
            .map_err(|e| MemoryError::Storage(format!("failed to load token projection: {}", e)))?;
            let rnn = BiLstmLayer::load(
                vb.pp("rnn"),
                runtime_config.head_hidden_size,
                runtime_config.head_hidden_size / 2,
            )
            .map_err(|e| MemoryError::Storage(format!("failed to load rnn: {}", e)))?;
            // Real weights use double prefix: span_rep_layer.span_rep_layer.*
            let span_rep_layer = SpanRepresentationLayer::load(
                vb.pp("span_rep_layer").pp("span_rep_layer"),
                runtime_config.head_hidden_size,
            )
            .map_err(|e| MemoryError::Storage(format!("failed to load span_rep_layer: {}", e)))?;
            let prompt_hidden = gliner_ffn_hidden_size(runtime_config.head_hidden_size);
            let prompt_rep_layer = FeedForwardProjection::load(
                vb.pp("prompt_rep_layer"),
                runtime_config.head_hidden_size,
                prompt_hidden,
                runtime_config.head_hidden_size,
            )
            .map_err(|e| MemoryError::Storage(format!("failed to load prompt_rep_layer: {}", e)))?;

            Ok(Self {
                model: std::sync::Arc::new(model),
                tokenizer: std::sync::Arc::new(tokenizer),
                device,
                labels,
                threshold,
                max_span_width: runtime_config.max_span_width,
                max_seq_len: runtime_config.max_seq_len,
                ent_token_id,
                sep_token_id,
                token_projection: std::sync::Arc::new(token_projection),
                rnn: std::sync::Arc::new(rnn),
                span_rep_layer: std::sync::Arc::new(span_rep_layer),
                prompt_rep_layer: std::sync::Arc::new(prompt_rep_layer),
            })
        } else {
            Err(MemoryError::Storage(
                "no model weights found (expected model.safetensors or pytorch_model.bin)"
                    .to_string(),
            ))
        }
    }

    fn resolve_ent_token(tokenizer: &Tokenizer) -> Result<u32, MemoryError> {
        for token in ENT_TOKEN_CANDIDATES {
            if let Some(id) = tokenizer.token_to_id(token) {
                return Ok(id);
            }
        }
        Err(MemoryError::Storage(format!(
            "GLiNER tokenizer missing entity separator token. Expected one of: {:?}",
            ENT_TOKEN_CANDIDATES
        )))
    }

    fn encode_prompt(&self, labels: &[String]) -> Result<PromptEncoding, MemoryError> {
        let mut tokens = Vec::new();
        let mut entity_token_positions = Vec::with_capacity(labels.len());
        for label in labels {
            entity_token_positions.push(tokens.len());
            tokens.push(self.ent_token_id);

            let encoding = self
                .tokenizer
                .encode(label.as_str(), false)
                .map_err(|e| MemoryError::Storage(format!("label tokenization failed: {}", e)))?;
            let label_ids: Vec<u32> = encoding.get_ids().to_vec();
            if label_ids.is_empty() {
                return Err(MemoryError::Storage(format!(
                    "label `{label}` produced no tokens"
                )));
            }
            tokens.extend_from_slice(&label_ids);
            tokens.push(self.sep_token_id);
        }
        Ok(PromptEncoding {
            token_ids: tokens,
            entity_token_positions,
        })
    }

    fn run_forward(&self, input_ids: &[u32]) -> Result<Tensor, MemoryError> {
        let attention_mask: Vec<u32> = vec![1u32; input_ids.len()];

        let input_ids = Tensor::new(input_ids, &self.device)
            .map_err(|e| MemoryError::Storage(format!("tensor error: {}", e)))?
            .unsqueeze(0)
            .map_err(|e| MemoryError::Storage(format!("unsqueeze error: {}", e)))?;

        let attention_mask = Tensor::new(attention_mask, &self.device)
            .map_err(|e| MemoryError::Storage(format!("mask tensor error: {}", e)))?
            .unsqueeze(0)
            .map_err(|e| MemoryError::Storage(format!("mask unsqueeze error: {}", e)))?;

        let type_ids = Tensor::zeros_like(&input_ids)
            .map_err(|e| MemoryError::Storage(format!("type_ids error: {}", e)))?;

        let hidden = self
            .model
            .forward(&input_ids, Some(type_ids), Some(attention_mask))
            .map_err(|e| MemoryError::Storage(format!("forward pass failed: {}", e)))?
            .squeeze(0)
            .map_err(|e| MemoryError::Storage(format!("squeeze failed: {}", e)))?;

        self.token_projection
            .forward(&hidden)
            .map_err(|e| MemoryError::Storage(format!("token projection failed: {}", e)))
    }

    fn build_label_representations(
        &self,
        prompt_hidden: &Tensor,
        prompt_encoding: &PromptEncoding,
    ) -> Result<Tensor, MemoryError> {
        let mut prompt_labels = Vec::with_capacity(prompt_encoding.entity_token_positions.len());

        for &entity_pos in &prompt_encoding.entity_token_positions {
            let label_hidden = prompt_hidden
                .narrow(0, entity_pos, 1)
                .map_err(|e| MemoryError::Storage(format!("label narrow failed: {}", e)))?
                .squeeze(0)
                .map_err(|e| MemoryError::Storage(format!("label squeeze failed: {}", e)))?;
            prompt_labels.push(label_hidden);
        }

        let prompt_label_refs = prompt_labels.iter().collect::<Vec<_>>();
        let prompt_label_embeddings = Tensor::stack(&prompt_label_refs, 0)
            .map_err(|e| MemoryError::Storage(format!("label stack failed: {}", e)))?;

        self.prompt_rep_layer
            .forward(&prompt_label_embeddings)
            .map_err(|e| MemoryError::Storage(format!("prompt projection failed: {}", e)))
    }

    #[allow(clippy::needless_range_loop)]
    fn compute_span_scores(
        &self,
        text_hidden: &Tensor,
        label_representations: &Tensor,
    ) -> Result<Vec<(usize, usize, Vec<f32>)>, MemoryError> {
        let text_len = text_hidden
            .dim(0)
            .map_err(|e| MemoryError::Storage(format!("dim error: {}", e)))?;
        let mut spans: Vec<(usize, usize, Vec<f32>)> = Vec::new();

        for start in 0..text_len {
            for end in start..std::cmp::min(start + self.max_span_width, text_len) {
                let h_start = text_hidden
                    .narrow(0, start, 1)
                    .map_err(|e| MemoryError::Storage(format!("narrow error: {}", e)))?;
                let h_end = text_hidden
                    .narrow(0, end, 1)
                    .map_err(|e| MemoryError::Storage(format!("narrow error: {}", e)))?;

                let span_proj = self
                    .span_rep_layer
                    .forward(&h_start, &h_end)
                    .map_err(|e| MemoryError::Storage(format!("span projection error: {}", e)))?;

                let scores = span_proj
                    .matmul(
                        &label_representations
                            .t()
                            .map_err(|e| MemoryError::Storage(format!("transpose error: {}", e)))?,
                    )
                    .map_err(|e| MemoryError::Storage(format!("matmul error: {}", e)))?;

                let scores: Vec<f32> = scores
                    .to_vec2::<f32>()
                    .map_err(|e| MemoryError::Storage(format!("to_vec2 error: {}", e)))?[0]
                    .clone();

                spans.push((start, end, scores));
            }
        }

        Ok(spans)
    }

    fn is_valid_span_text(span_text: &str) -> bool {
        !span_text.trim().is_empty()
    }

    fn extract_spans(
        &self,
        text: &str,
        spans_data: &[(usize, usize, Vec<f32>)],
        offsets: &[(usize, usize)],
    ) -> Vec<ScoredSpan> {
        let mut spans: Vec<ScoredSpan> = Vec::new();

        for &(start, end, ref scores) in spans_data.iter() {
            if start >= offsets.len() || end >= offsets.len() {
                continue;
            }

            let start_char = offsets[start].0;
            let end_char = offsets[end].1;

            if end_char <= start_char || end_char > text.len() {
                continue;
            }

            let span_text = &text[start_char..end_char];
            let span_text = span_text.trim();

            if !Self::is_valid_span_text(span_text) {
                continue;
            }

            for (label_idx, &score) in scores.iter().enumerate() {
                if label_idx >= self.labels.len() {
                    break;
                }
                // Apply sigmoid to convert logit to probability before threshold comparison.
                let prob = 1.0_f32 / (1.0_f32 + (-score).exp());
                if prob >= self.threshold as f32 {
                    spans.push(ScoredSpan {
                        start: start_char,
                        end: end_char,
                        text: span_text.to_string(),
                        label: self.labels[label_idx].clone(),
                        score: prob,
                    });
                }
            }
        }

        spans
    }

    /// Apply label-aware non-maximum suppression with IoU threshold.
    ///
    /// Overlaps are suppressed only within the same label type.
    /// Spans with IoU above 0.5 are suppressed; spans with lower overlap coexist.
    /// E.g., "New York" and "New York City" can both survive if their IoU <= 0.5.
    fn apply_nms(&self, mut spans: Vec<ScoredSpan>) -> Vec<ScoredSpan> {
        const IOU_THRESHOLD: f32 = 0.5;

        spans.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        let mut kept: Vec<ScoredSpan> = Vec::new();

        for span in spans {
            let dominated = kept.iter().any(|k| {
                if k.label != span.label {
                    return false;
                }
                let inter_start = span.start.max(k.start);
                let inter_end = span.end.min(k.end);
                if inter_start >= inter_end {
                    return false;
                }
                let intersection = (inter_end - inter_start) as f32;
                let union = (span.end - span.start + k.end - k.start) as f32 - intersection;
                intersection / union > IOU_THRESHOLD
            });

            if !dominated {
                kept.push(span);
            }
        }

        kept
    }

    fn extract_inner(&self, text: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        let prompt_encoding = self.encode_prompt(&self.labels)?;

        let max_text_tokens = self
            .max_seq_len
            .saturating_sub(prompt_encoding.token_ids.len());
        if max_text_tokens == 0 {
            return Err(MemoryError::Storage(
                "prompt too long for MAX_SEQ_LEN".to_string(),
            ));
        }

        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| MemoryError::Storage(format!("tokenization failed: {}", e)))?;

        let text_ids: Vec<u32> = encoding.get_ids().to_vec();
        let offsets: Vec<(usize, usize)> = encoding.get_offsets().to_vec();

        let mut all_spans: Vec<ScoredSpan> = Vec::new();

        // Use ~12.5% overlap between windows so entities spanning chunk boundaries
        // are still captured. With max_seq_len=384 this gives ~48 token overlap.
        let overlap = max_text_tokens / 8;
        let step = max_text_tokens.saturating_sub(overlap).max(1);
        for window_start in (0..text_ids.len()).step_by(step) {
            let window_end = std::cmp::min(window_start + max_text_tokens, text_ids.len());
            let window_ids: Vec<u32> = text_ids[window_start..window_end].to_vec();

            let window_offsets: Vec<(usize, usize)> = if window_start < offsets.len() {
                offsets[window_start..std::cmp::min(window_end, offsets.len())].to_vec()
            } else {
                Vec::new()
            };

            let mut input_ids = prompt_encoding.token_ids.clone();
            input_ids.extend_from_slice(&window_ids);

            let hidden = self.run_forward(&input_ids)?;

            let prompt_len = prompt_encoding.token_ids.len();
            let prompt_hidden = hidden
                .narrow(0, 0, prompt_len)
                .map_err(|e| MemoryError::Storage(format!("prompt narrow failed: {}", e)))?;
            let label_representations =
                self.build_label_representations(&prompt_hidden, &prompt_encoding)?;

            let text_hidden = if !window_ids.is_empty() {
                let text_hidden = hidden
                    .narrow(0, prompt_len, window_ids.len())
                    .map_err(|e| MemoryError::Storage(format!("narrow failed: {}", e)))?;
                self.rnn
                    .forward(&text_hidden)
                    .map_err(|e| MemoryError::Storage(format!("rnn forward failed: {}", e)))?
            } else {
                continue;
            };

            let spans_data = self.compute_span_scores(&text_hidden, &label_representations)?;

            let window_spans = self.extract_spans(text, &spans_data, &window_offsets);
            all_spans.extend(window_spans);
        }

        let final_spans = self.apply_nms(all_spans);

        let mut candidates: Vec<EntityCandidate> = final_spans
            .into_iter()
            .map(|span| EntityCandidate {
                entity_type: span.label,
                canonical_name: span.text,
                aliases: Vec::new(),
            })
            .collect();

        candidates.sort_by(|a, b| a.canonical_name.cmp(&b.canonical_name));
        candidates.dedup_by(|a, b| {
            a.canonical_name == b.canonical_name && a.entity_type == b.entity_type
        });

        Ok(candidates)
    }
}

impl std::fmt::Debug for GlinerEntityExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlinerEntityExtractor")
            .field("labels", &self.labels)
            .field("threshold", &self.threshold)
            .finish()
    }
}

#[async_trait]
impl EntityExtractor for GlinerEntityExtractor {
    fn provider_name(&self) -> &'static str {
        "gliner"
    }

    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        let extractor = self.clone();
        let content = content.to_string();
        tokio::task::spawn_blocking(move || extractor.extract_inner(&content))
            .await
            .map_err(|e| MemoryError::Storage(format!("NER task panicked: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{DType, Device};
    use serde_json::json;

    fn metadata_entry(shape: &[usize]) -> SafetensorsTensorMetadata {
        SafetensorsTensorMetadata {
            shape: shape.to_vec(),
        }
    }

    #[test]
    fn infers_backbone_config_from_actual_gliner_weight_layout() {
        let metadata = HashMap::from([
            (
                format!("{BACKBONE_PREFIX}.embeddings.word_embeddings.weight"),
                metadata_entry(&[250_105, 768]),
            ),
            (
                format!("{BACKBONE_PREFIX}.encoder.layer.0.intermediate.dense.weight"),
                metadata_entry(&[3072, 768]),
            ),
            (
                format!("{BACKBONE_PREFIX}.encoder.layer.11.intermediate.dense.weight"),
                metadata_entry(&[3072, 768]),
            ),
            (
                format!("{BACKBONE_PREFIX}.encoder.rel_embeddings.weight"),
                metadata_entry(&[512, 768]),
            ),
            (
                format!("{BACKBONE_PREFIX}.encoder.LayerNorm.weight"),
                metadata_entry(&[768]),
            ),
        ]);

        let config = infer_backbone_config_from_metadata(&metadata, 384, 0.4)
            .expect("infer backbone config");

        assert_eq!(config.hidden_size, 768);
        assert_eq!(config.num_hidden_layers, 12);
        assert_eq!(config.num_attention_heads, 12);
        assert_eq!(config.intermediate_size, 3072);
        assert_eq!(config.vocab_size, 250_105);
        assert_eq!(config.type_vocab_size, 0);
        assert!(!config.position_biased_input);
        assert_eq!(config.position_buckets, Some(256));
        assert_eq!(config.share_att_key, Some(true));
        assert_eq!(config.norm_rel_ebd.as_deref(), Some("layer_norm"));
        assert_eq!(config.max_position_embeddings, 512);
    }

    #[test]
    fn gliner_projection_heads_load_actual_prefixes() {
        let device = Device::Cpu;
        let tensors = HashMap::from([
            (
                "token_rep_layer.projection.weight".to_string(),
                Tensor::zeros((4, 6), DType::F32, &device).expect("projection weight"),
            ),
            (
                "token_rep_layer.projection.bias".to_string(),
                Tensor::zeros(4, DType::F32, &device).expect("projection bias"),
            ),
            (
                "prompt_rep_layer.0.weight".to_string(),
                Tensor::zeros((16, 4), DType::F32, &device).expect("prompt 0 weight"),
            ),
            (
                "prompt_rep_layer.0.bias".to_string(),
                Tensor::zeros(16, DType::F32, &device).expect("prompt 0 bias"),
            ),
            (
                "prompt_rep_layer.3.weight".to_string(),
                Tensor::zeros((4, 16), DType::F32, &device).expect("prompt 3 weight"),
            ),
            (
                "prompt_rep_layer.3.bias".to_string(),
                Tensor::zeros(4, DType::F32, &device).expect("prompt 3 bias"),
            ),
            (
                "span_rep_layer.project_start.0.weight".to_string(),
                Tensor::zeros((16, 4), DType::F32, &device).expect("start 0 weight"),
            ),
            (
                "span_rep_layer.project_start.0.bias".to_string(),
                Tensor::zeros(16, DType::F32, &device).expect("start 0 bias"),
            ),
            (
                "span_rep_layer.project_start.3.weight".to_string(),
                Tensor::zeros((4, 16), DType::F32, &device).expect("start 3 weight"),
            ),
            (
                "span_rep_layer.project_start.3.bias".to_string(),
                Tensor::zeros(4, DType::F32, &device).expect("start 3 bias"),
            ),
            (
                "span_rep_layer.project_end.0.weight".to_string(),
                Tensor::zeros((16, 4), DType::F32, &device).expect("end 0 weight"),
            ),
            (
                "span_rep_layer.project_end.0.bias".to_string(),
                Tensor::zeros(16, DType::F32, &device).expect("end 0 bias"),
            ),
            (
                "span_rep_layer.project_end.3.weight".to_string(),
                Tensor::zeros((4, 16), DType::F32, &device).expect("end 3 weight"),
            ),
            (
                "span_rep_layer.project_end.3.bias".to_string(),
                Tensor::zeros(4, DType::F32, &device).expect("end 3 bias"),
            ),
            (
                "span_rep_layer.out_project.0.weight".to_string(),
                Tensor::zeros((16, 8), DType::F32, &device).expect("out 0 weight"),
            ),
            (
                "span_rep_layer.out_project.0.bias".to_string(),
                Tensor::zeros(16, DType::F32, &device).expect("out 0 bias"),
            ),
            (
                "span_rep_layer.out_project.3.weight".to_string(),
                Tensor::zeros((4, 16), DType::F32, &device).expect("out 3 weight"),
            ),
            (
                "span_rep_layer.out_project.3.bias".to_string(),
                Tensor::zeros(4, DType::F32, &device).expect("out 3 bias"),
            ),
        ]);
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);

        TokenProjectionLayer::load(vb.pp("token_rep_layer"), 6, 4).expect("load token projection");
        FeedForwardProjection::load(vb.pp("prompt_rep_layer"), 4, 16, 4)
            .expect("load prompt rep layer");
        SpanRepresentationLayer::load(vb.pp("span_rep_layer"), 4).expect("load span rep layer");
    }

    #[test]
    fn prompt_rep_layer_loads_actual_shape() {
        let device = Device::Cpu;
        let tensors = HashMap::from([
            (
                "prompt_rep_layer.0.weight".to_string(),
                Tensor::zeros((2048, 512), DType::F32, &device).expect("prompt 0 weight"),
            ),
            (
                "prompt_rep_layer.0.bias".to_string(),
                Tensor::zeros(2048, DType::F32, &device).expect("prompt 0 bias"),
            ),
            (
                "prompt_rep_layer.3.weight".to_string(),
                Tensor::zeros((512, 2048), DType::F32, &device).expect("prompt 3 weight"),
            ),
            (
                "prompt_rep_layer.3.bias".to_string(),
                Tensor::zeros(512, DType::F32, &device).expect("prompt 3 bias"),
            ),
        ]);
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);

        FeedForwardProjection::load(
            vb.pp("prompt_rep_layer"),
            512,
            gliner_ffn_hidden_size(512),
            512,
        )
        .expect("load prompt rep layer with actual GLiNER shape");
    }

    #[test]
    fn bilstm_layer_loads_actual_gliner_prefixes() {
        let device = Device::Cpu;
        let tensors = HashMap::from([
            (
                "rnn.lstm.weight_ih_l0".to_string(),
                Tensor::zeros((8, 4), DType::F32, &device).expect("forward ih weight"),
            ),
            (
                "rnn.lstm.weight_hh_l0".to_string(),
                Tensor::zeros((8, 2), DType::F32, &device).expect("forward hh weight"),
            ),
            (
                "rnn.lstm.bias_ih_l0".to_string(),
                Tensor::zeros(8, DType::F32, &device).expect("forward ih bias"),
            ),
            (
                "rnn.lstm.bias_hh_l0".to_string(),
                Tensor::zeros(8, DType::F32, &device).expect("forward hh bias"),
            ),
            (
                "rnn.lstm.weight_ih_l0_reverse".to_string(),
                Tensor::zeros((8, 4), DType::F32, &device).expect("backward ih weight"),
            ),
            (
                "rnn.lstm.weight_hh_l0_reverse".to_string(),
                Tensor::zeros((8, 2), DType::F32, &device).expect("backward hh weight"),
            ),
            (
                "rnn.lstm.bias_ih_l0_reverse".to_string(),
                Tensor::zeros(8, DType::F32, &device).expect("backward ih bias"),
            ),
            (
                "rnn.lstm.bias_hh_l0_reverse".to_string(),
                Tensor::zeros(8, DType::F32, &device).expect("backward hh bias"),
            ),
        ]);
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);

        BiLstmLayer::load(vb.pp("rnn"), 4, 2).expect("load GLiNER biLSTM layer");
    }

    #[test]
    fn span_text_filter_keeps_punctuation_separated_terms() {
        assert!(GlinerEntityExtractor::is_valid_span_text("API/SDK"));
        assert!(GlinerEntityExtractor::is_valid_span_text("v2.1"));
        assert!(!GlinerEntityExtractor::is_valid_span_text("   "));
    }

    #[test]
    fn parses_gliner_runtime_config_with_model_name_fallback() {
        let runtime = parse_gliner_runtime_config(
            &json!({
                "hidden_size": 512,
                "max_len": 384,
                "dropout": 0.4,
                "model_name": "microsoft/mdeberta-v3-base",
                "max_width": 12
            })
            .to_string(),
            None,
        )
        .expect("parse runtime config");

        assert_eq!(runtime.head_hidden_size, 512);
        assert_eq!(runtime.max_span_width, 12);
        assert_eq!(runtime.max_seq_len, 384);
        assert_eq!(runtime.backbone.hidden_size, 768);
        assert_eq!(runtime.backbone.num_hidden_layers, 12);
        assert_eq!(runtime.backbone.num_attention_heads, 12);
        assert_eq!(runtime.backbone.share_att_key, Some(true));
        assert!(!runtime.backbone.position_biased_input);
    }
}
