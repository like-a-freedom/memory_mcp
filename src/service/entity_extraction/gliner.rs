//! GLiNER-based NER extractor using Candle inference.
//!
//! This module provides zero-shot NER extraction with local GLiNER weights.

use std::{collections::HashMap, path::Path, sync::LazyLock};

use async_trait::async_trait;
use candle_core::{Device, IndexOp, Module, Tensor};
use candle_nn::rnn::Direction;
use candle_nn::{LSTM, LSTMConfig, RNN, VarBuilder};
use candle_transformers::models::debertav2::{Config, DTYPE, DebertaV2Model};
use tokenizers::{Encoding, Tokenizer};

use crate::models::EntityCandidate;

use super::{EntityExtractor, MemoryError};

mod batching;
mod gate;
mod scoring;

const ENT_TOKEN_CANDIDATES: &[&str] = &["<<ENT>>", "[ENT]", "<<SEP>>", "@"];
const SEP_TOKEN: &str = "<<SEP>>";
const DEFAULT_MAX_SPAN_WIDTH: usize = 12;
const DEFAULT_MAX_SEQ_LEN: usize = 384;
const FALLBACK_BACKBONE_MAX_POSITION_EMBEDDINGS: usize = 512;
const BACKBONE_PREFIX: &str = "token_rep_layer.bert_layer.model";

static WHITESPACE_WORD_SPLITTER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\w+(?:[-_]\w+)*|\S").expect("valid splitter regex"));

#[derive(Debug, Clone)]
struct ScoredSpan {
    start: usize,
    end: usize,
    text: String,
    label: String,
    score: f32,
}

pub struct GlinerEntityExtractor {
    model: DebertaV2Model,
    tokenizer: Tokenizer,
    device: Device,
    labels: Vec<String>,
    threshold: f64,
    max_span_width: usize,
    max_seq_len: usize,
    ent_token_id: u32,
    token_projection: TokenProjectionLayer,
    rnn: BiLstmLayer,
    span_rep_layer: SpanRepresentationLayer,
    prompt_rep_layer: FeedForwardProjection,
    logger: crate::logging::StdoutLogger,
    batch_size: usize,
    max_batch_tokens: usize,
    inference_gate: gate::InferenceGate,
}

#[derive(Debug, Clone)]
struct GlinerRuntimeConfig {
    backbone: Config,
    head_hidden_size: usize,
    max_span_width: usize,
    max_seq_len: usize,
}

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
        let projection = if input_dim == output_dim {
            match candle_nn::linear(input_dim, output_dim, vb.pp("projection")) {
                Ok(linear) => Some(linear),
                Err(candle_core::Error::CannotFindTensor { .. }) => None,
                Err(err) => return Err(err),
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
    let config: GlinerConfig = serde_json::from_str(json_str)
        .map_err(|err| MemoryError::Storage(format!("failed to parse GLiNER config: {err}")))?;
    let backbone = match safetensors_path {
        Some(path) => {
            let metadata = read_safetensors_metadata(path)?;
            infer_backbone_config_from_metadata(
                &metadata,
                config.max_position_embeddings,
                config.hidden_dropout_prob,
            )?
        }
        None => infer_backbone_config_from_model_name(
            config.model_name.as_deref(),
            config.max_position_embeddings,
            config.hidden_dropout_prob,
        )?,
    };

    Ok(GlinerRuntimeConfig {
        backbone,
        head_hidden_size: config.hidden_size,
        max_span_width: config.max_span_width,
        max_seq_len: config.max_position_embeddings.max(DEFAULT_MAX_SEQ_LEN),
    })
}

fn read_safetensors_metadata(
    path: &Path,
) -> Result<HashMap<String, SafetensorsTensorMetadata>, MemoryError> {
    let bytes = std::fs::read(path)
        .map_err(|err| MemoryError::Storage(format!("failed to read safetensors header: {err}")))?;
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

    serde_json::from_slice(&bytes[header_start..header_end]).map_err(|err| {
        MemoryError::Storage(format!(
            "failed to parse safetensors header metadata: {err}"
        ))
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
        Self::new_with_logger(
            model_dir,
            labels,
            threshold,
            crate::logging::StdoutLogger::new("warn"),
        )
    }

    pub(crate) fn new_with_logger(
        model_dir: &Path,
        labels: Vec<String>,
        threshold: f64,
        logger: crate::logging::StdoutLogger,
    ) -> Result<Self, MemoryError> {
        Self::new_with_runtime(
            model_dir,
            labels,
            threshold,
            crate::config::DEFAULT_NER_BATCH_SIZE,
            crate::config::DEFAULT_NER_MAX_BATCH_TOKENS,
            crate::config::DEFAULT_NER_MAX_CONCURRENCY,
            logger,
        )
    }

    pub(crate) fn new_with_runtime(
        model_dir: &Path,
        labels: Vec<String>,
        threshold: f64,
        batch_size: usize,
        max_batch_tokens: usize,
        max_concurrency: usize,
        logger: crate::logging::StdoutLogger,
    ) -> Result<Self, MemoryError> {
        if batch_size == 0 || max_batch_tokens == 0 {
            return Err(MemoryError::ConfigInvalid(
                "NER batch limits must be greater than zero".to_string(),
            ));
        }
        if max_concurrency == 0 {
            return Err(MemoryError::ConfigInvalid(
                "NER_MAX_CONCURRENCY must be greater than zero".to_string(),
            ));
        }
        Self::load_with_runtime(
            model_dir,
            labels,
            threshold,
            batch_size,
            max_batch_tokens,
            max_concurrency,
            logger,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn load_with_runtime(
        model_dir: &Path,
        labels: Vec<String>,
        threshold: f64,
        batch_size: usize,
        max_batch_tokens: usize,
        max_concurrency: usize,
        logger: crate::logging::StdoutLogger,
    ) -> Result<Self, MemoryError> {
        let tokenizer_path = model_dir.join("tokenizer.json");
        let config_path = if model_dir.join("gliner_config.json").exists() {
            model_dir.join("gliner_config.json")
        } else {
            model_dir.join("config.json")
        };

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|err| MemoryError::Storage(format!("failed to load tokenizer: {err}")))?;

        let config_str = std::fs::read_to_string(&config_path)
            .map_err(|err| MemoryError::Storage(format!("failed to read config: {err}")))?;

        let safetensors_path = model_dir.join("model.safetensors");
        let pytorch_path = model_dir.join("pytorch_model.bin");

        let runtime_config = if config_path
            .file_name()
            .map(|name| name == "gliner_config.json")
            .unwrap_or(false)
        {
            parse_gliner_runtime_config(
                &config_str,
                safetensors_path
                    .is_file()
                    .then_some(safetensors_path.as_path()),
            )
            .map_err(|err| MemoryError::Storage(format!("failed to parse config: {err}")))?
        } else {
            let backbone: Config = serde_json::from_str(&config_str)
                .map_err(|err| MemoryError::Storage(format!("failed to parse config: {err}")))?;
            GlinerRuntimeConfig {
                head_hidden_size: backbone.hidden_size,
                max_span_width: DEFAULT_MAX_SPAN_WIDTH,
                max_seq_len: backbone.max_position_embeddings.max(DEFAULT_MAX_SEQ_LEN),
                backbone,
            }
        };

        let device = Device::Cpu;

        let vb = if safetensors_path.is_file() {
            unsafe { VarBuilder::from_mmaped_safetensors(&[&safetensors_path], DTYPE, &device) }
                .map_err(|err| MemoryError::Storage(format!("failed to load safetensors: {err}")))?
        } else if pytorch_path.is_file() {
            VarBuilder::from_pth(pytorch_path.to_str().unwrap_or(""), DTYPE, &device).map_err(
                |err| MemoryError::Storage(format!("failed to load pytorch weights: {err}")),
            )?
        } else {
            return Err(MemoryError::Storage(
                "no model weights found (expected model.safetensors or pytorch_model.bin)"
                    .to_string(),
            ));
        };

        Self::build_from_var_builder(
            tokenizer,
            vb,
            &device,
            runtime_config,
            labels,
            threshold,
            logger,
            batch_size,
            max_batch_tokens,
            max_concurrency,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_from_var_builder(
        tokenizer: Tokenizer,
        vb: VarBuilder,
        device: &Device,
        runtime_config: GlinerRuntimeConfig,
        labels: Vec<String>,
        threshold: f64,
        logger: crate::logging::StdoutLogger,
        batch_size: usize,
        max_batch_tokens: usize,
        max_concurrency: usize,
    ) -> Result<Self, MemoryError> {
        let ent_token_id = Self::resolve_ent_token(&tokenizer)?;

        let model = DebertaV2Model::load(vb.pp(BACKBONE_PREFIX), &runtime_config.backbone)
            .map_err(|err| MemoryError::Storage(format!("failed to build model: {err}")))?;
        let token_projection = TokenProjectionLayer::load(
            vb.pp("token_rep_layer"),
            runtime_config.backbone.hidden_size,
            runtime_config.head_hidden_size,
        )
        .map_err(|err| MemoryError::Storage(format!("failed to load token projection: {err}")))?;
        let rnn = BiLstmLayer::load(
            vb.pp("rnn"),
            runtime_config.head_hidden_size,
            runtime_config.head_hidden_size / 2,
        )
        .map_err(|err| MemoryError::Storage(format!("failed to load rnn: {err}")))?;
        let span_rep_layer = SpanRepresentationLayer::load(
            vb.pp("span_rep_layer").pp("span_rep_layer"),
            runtime_config.head_hidden_size,
        )
        .map_err(|err| MemoryError::Storage(format!("failed to load span_rep_layer: {err}")))?;
        let prompt_hidden = gliner_ffn_hidden_size(runtime_config.head_hidden_size);
        let prompt_rep_layer = FeedForwardProjection::load(
            vb.pp("prompt_rep_layer"),
            runtime_config.head_hidden_size,
            prompt_hidden,
            runtime_config.head_hidden_size,
        )
        .map_err(|err| MemoryError::Storage(format!("failed to load prompt_rep_layer: {err}")))?;

        Ok(Self {
            model,
            tokenizer,
            device: device.clone(),
            labels,
            threshold,
            max_span_width: runtime_config.max_span_width,
            max_seq_len: runtime_config.max_seq_len,
            ent_token_id,
            token_projection,
            rnn,
            span_rep_layer,
            prompt_rep_layer,
            logger,
            batch_size,
            max_batch_tokens,
            inference_gate: gate::InferenceGate::new(max_concurrency),
        })
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

    fn split_text_words(text: &str) -> Vec<(String, (usize, usize))> {
        WHITESPACE_WORD_SPLITTER
            .find_iter(text)
            .map(|mat| (mat.as_str().to_string(), (mat.start(), mat.end())))
            .collect()
    }

    fn encode_window(
        &self,
        prompt_words: &[String],
        text_words: &[(String, (usize, usize))],
        window_start: usize,
    ) -> Result<(Encoding, usize), MemoryError> {
        let mut last_fit = None;

        for window_end in window_start + 1..=text_words.len() {
            let mut input_words =
                Vec::with_capacity(prompt_words.len() + window_end - window_start);
            input_words.extend(prompt_words.iter().cloned());
            input_words.extend(
                text_words[window_start..window_end]
                    .iter()
                    .map(|(word, _)| word.clone()),
            );

            let encoding = self
                .tokenizer
                .encode(input_words, true)
                .map_err(|err| MemoryError::Storage(format!("tokenization failed: {err}")))?;

            if encoding.len() > self.max_seq_len {
                break;
            }

            last_fit = Some((encoding, window_end));
        }

        last_fit.ok_or_else(|| {
            MemoryError::Storage(format!(
                "GLiNER input window does not fit into max sequence length {}",
                self.max_seq_len
            ))
        })
    }

    fn collect_prompt_entity_positions(
        &self,
        input_ids: &[u32],
        word_ids: &[Option<u32>],
        prompt_word_count: usize,
    ) -> Vec<usize> {
        input_ids
            .iter()
            .enumerate()
            .filter_map(|(index, token_id)| {
                (token_id == &self.ent_token_id
                    && word_ids
                        .get(index)
                        .and_then(|word_id| *word_id)
                        .is_some_and(|word_id| word_id < prompt_word_count as u32))
                .then_some(index)
            })
            .collect()
    }

    fn extract_word_representations(
        &self,
        hidden: &Tensor,
        word_ids: &[Option<u32>],
        prompt_word_count: usize,
        text_offsets: &[(usize, usize)],
    ) -> Result<(Tensor, Vec<(usize, usize)>), MemoryError> {
        let mut prev_word_id = None;
        let mut word_states = Vec::new();
        let mut word_offsets = Vec::new();

        for (token_index, word_id) in word_ids.iter().enumerate() {
            let Some(word_id) = *word_id else {
                prev_word_id = None;
                continue;
            };

            if Some(word_id) == prev_word_id {
                continue;
            }
            prev_word_id = Some(word_id);

            if word_id < prompt_word_count as u32 {
                continue;
            }

            let text_word_index = (word_id as usize).saturating_sub(prompt_word_count);
            if text_word_index >= text_offsets.len() {
                continue;
            }

            let word_hidden = hidden
                .narrow(0, token_index, 1)
                .map_err(|err| MemoryError::Storage(format!("word narrow failed: {err}")))?
                .squeeze(0)
                .map_err(|err| MemoryError::Storage(format!("word squeeze failed: {err}")))?;
            word_states.push(word_hidden);
            word_offsets.push(text_offsets[text_word_index]);
        }

        if word_states.is_empty() {
            return Err(MemoryError::Storage(
                "GLiNER tokenization produced no word-level text embeddings".to_string(),
            ));
        }

        let word_state_refs = word_states.iter().collect::<Vec<_>>();
        let word_tensor = Tensor::stack(&word_state_refs, 0)
            .map_err(|err| MemoryError::Storage(format!("word stack failed: {err}")))?;

        Ok((word_tensor, word_offsets))
    }

    #[allow(dead_code)]
    fn run_forward(&self, input_ids: &[u32]) -> Result<Tensor, MemoryError> {
        let attention_mask = vec![1u32; input_ids.len()];

        let input_ids = Tensor::new(input_ids, &self.device)
            .map_err(|err| MemoryError::Storage(format!("tensor error: {err}")))?
            .unsqueeze(0)
            .map_err(|err| MemoryError::Storage(format!("unsqueeze error: {err}")))?;
        let attention_mask = Tensor::new(attention_mask, &self.device)
            .map_err(|err| MemoryError::Storage(format!("mask tensor error: {err}")))?
            .unsqueeze(0)
            .map_err(|err| MemoryError::Storage(format!("mask unsqueeze error: {err}")))?;
        let type_ids = Tensor::zeros_like(&input_ids)
            .map_err(|err| MemoryError::Storage(format!("type_ids error: {err}")))?;

        let hidden = self
            .model
            .forward(&input_ids, Some(type_ids), Some(attention_mask))
            .map_err(|err| MemoryError::Storage(format!("forward pass failed: {err}")))?
            .squeeze(0)
            .map_err(|err| MemoryError::Storage(format!("squeeze failed: {err}")))?;

        self.token_projection
            .forward(&hidden)
            .map_err(|err| MemoryError::Storage(format!("token projection failed: {err}")))
    }

    fn run_forward_batch(
        &self,
        windows: &[batching::EncodedWindow],
    ) -> Result<Vec<Tensor>, MemoryError> {
        let batch_size = windows.len();
        let max_len = windows
            .iter()
            .map(|window| window.input_ids.len())
            .max()
            .unwrap_or(0);
        let pad_id = self
            .tokenizer
            .get_padding()
            .map_or(0, |padding| padding.pad_id);
        let mut ids = vec![vec![pad_id; max_len]; batch_size];
        let mut masks = vec![vec![0u32; max_len]; batch_size];
        for (row, window) in windows.iter().enumerate() {
            ids[row][..window.input_ids.len()].copy_from_slice(&window.input_ids);
            masks[row][..window.input_ids.len()].fill(1);
        }
        let input_ids = Tensor::new(ids, &self.device)
            .map_err(|err| MemoryError::Storage(format!("batched input tensor failed: {err}")))?;
        let attention_mask = Tensor::new(masks, &self.device)
            .map_err(|err| MemoryError::Storage(format!("batched mask tensor failed: {err}")))?;
        let type_ids = Tensor::zeros_like(&input_ids)
            .map_err(|err| MemoryError::Storage(format!("batched type ids failed: {err}")))?;
        let hidden = self
            .model
            .forward(&input_ids, Some(type_ids), Some(attention_mask))
            .map_err(|err| MemoryError::Storage(format!("batched forward pass failed: {err}")))?;
        let projected = self.token_projection.forward(&hidden).map_err(|err| {
            MemoryError::Storage(format!("batched token projection failed: {err}"))
        })?;
        windows
            .iter()
            .enumerate()
            .map(|(row, window)| {
                projected
                    .narrow(0, row, 1)
                    .and_then(|tensor| tensor.squeeze(0))
                    .and_then(|tensor| tensor.narrow(0, 0, window.input_ids.len()))
                    .map_err(|err| {
                        MemoryError::Storage(format!("batched hidden split failed: {err}"))
                    })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_window(
        &self,
        text: &str,
        text_words: &[(String, (usize, usize))],
        labels: &[String],
        prompt_word_count: usize,
        window: &batching::EncodedWindow,
        hidden: &Tensor,
        all_spans: &mut Vec<ScoredSpan>,
    ) -> Result<(), MemoryError> {
        let entity_token_positions = self.collect_prompt_entity_positions(
            &window.input_ids,
            &window.word_ids,
            prompt_word_count,
        );
        if entity_token_positions.len() != labels.len() {
            return Err(MemoryError::Storage(format!(
                "GLiNER prompt extraction mismatch: expected {} entity tokens, found {}",
                labels.len(),
                entity_token_positions.len()
            )));
        }
        let label_representations =
            self.build_label_representations(hidden, &entity_token_positions)?;
        let window_offsets = text_words[window.window_start..window.window_end]
            .iter()
            .map(|(_, offsets)| *offsets)
            .collect::<Vec<_>>();
        let (word_hidden, word_offsets) = self.extract_word_representations(
            hidden,
            &window.word_ids,
            prompt_word_count,
            &window_offsets,
        )?;
        let text_hidden = self
            .rnn
            .forward(&word_hidden)
            .map_err(|err| MemoryError::Storage(format!("rnn forward failed: {err}")))?;
        let spans_data = self.compute_span_scores(&text_hidden, &label_representations)?;
        all_spans.extend(self.extract_spans(text, &spans_data, &word_offsets));
        Ok(())
    }

    fn build_label_representations(
        &self,
        hidden: &Tensor,
        entity_token_positions: &[usize],
    ) -> Result<Tensor, MemoryError> {
        let mut prompt_labels = Vec::with_capacity(entity_token_positions.len());

        for &entity_pos in entity_token_positions {
            let label_hidden = hidden
                .narrow(0, entity_pos, 1)
                .map_err(|err| MemoryError::Storage(format!("label narrow failed: {err}")))?
                .squeeze(0)
                .map_err(|err| MemoryError::Storage(format!("label squeeze failed: {err}")))?;
            prompt_labels.push(label_hidden);
        }

        let prompt_label_refs = prompt_labels.iter().collect::<Vec<_>>();
        let prompt_label_embeddings = Tensor::stack(&prompt_label_refs, 0)
            .map_err(|err| MemoryError::Storage(format!("label stack failed: {err}")))?;

        self.prompt_rep_layer
            .forward(&prompt_label_embeddings)
            .map_err(|err| MemoryError::Storage(format!("prompt projection failed: {err}")))
    }

    #[allow(clippy::needless_range_loop)]
    fn compute_span_scores(
        &self,
        text_hidden: &Tensor,
        label_representations: &Tensor,
    ) -> Result<Vec<(usize, usize, Vec<f32>)>, MemoryError> {
        let timer = std::time::Instant::now();
        let text_len = text_hidden
            .dim(0)
            .map_err(|err| MemoryError::Storage(format!("dim error: {err}")))?;
        let span_indices = scoring::enumerate_span_indices(text_len, self.max_span_width);
        if span_indices.is_empty() {
            return Ok(Vec::new());
        }

        let starts = span_indices
            .iter()
            .map(|span| span.start as u32)
            .collect::<Vec<_>>();
        let ends = span_indices
            .iter()
            .map(|span| span.end as u32)
            .collect::<Vec<_>>();
        let start_indices = Tensor::new(starts.as_slice(), &self.device)
            .map_err(|err| MemoryError::Storage(format!("start index tensor failed: {err}")))?;
        let end_indices = Tensor::new(ends.as_slice(), &self.device)
            .map_err(|err| MemoryError::Storage(format!("end index tensor failed: {err}")))?;
        let start_hidden = text_hidden
            .index_select(&start_indices, 0)
            .map_err(|err| MemoryError::Storage(format!("start gather failed: {err}")))?;
        let end_hidden = text_hidden
            .index_select(&end_indices, 0)
            .map_err(|err| MemoryError::Storage(format!("end gather failed: {err}")))?;
        let span_representations = self
            .span_rep_layer
            .forward(&start_hidden, &end_hidden)
            .map_err(|err| MemoryError::Storage(format!("span projection failed: {err}")))?;
        let label_transposed = label_representations
            .t()
            .map_err(|err| MemoryError::Storage(format!("label transpose failed: {err}")))?;
        let score_rows = span_representations
            .matmul(&label_transposed)
            .map_err(|err| MemoryError::Storage(format!("span score matmul failed: {err}")))?
            .to_vec2::<f32>()
            .map_err(|err| MemoryError::Storage(format!("span score transfer failed: {err}")))?;

        let spans = span_indices
            .into_iter()
            .zip(score_rows)
            .map(|(span, scores)| (span.start, span.end, scores))
            .collect::<Vec<_>>();
        self.logger.log(
            build_span_scoring_log_event(text_len, spans.len(), timer.elapsed()),
            crate::logging::LogLevel::Debug,
        );
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
        let mut spans = Vec::new();

        for &(start, end, ref scores) in spans_data {
            if start >= offsets.len() || end >= offsets.len() {
                continue;
            }

            let start_char = offsets[start].0;
            let end_char = offsets[end].1;
            if end_char <= start_char || end_char > text.len() {
                continue;
            }

            let span_text = text[start_char..end_char].trim();
            if !Self::is_valid_span_text(span_text) {
                continue;
            }

            for (label_idx, &score) in scores.iter().enumerate() {
                if label_idx >= self.labels.len() {
                    break;
                }
                let probability = 1.0_f32 / (1.0_f32 + (-score).exp());
                if probability >= self.threshold as f32 {
                    spans.push(ScoredSpan {
                        start: start_char,
                        end: end_char,
                        text: span_text.to_string(),
                        label: self.labels[label_idx].clone(),
                        score: probability,
                    });
                }
            }
        }

        spans
    }

    fn apply_nms(&self, mut spans: Vec<ScoredSpan>) -> Vec<ScoredSpan> {
        const IOU_THRESHOLD: f32 = 0.5;

        spans.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut kept = Vec::new();
        for span in spans {
            let dominated = kept.iter().any(|kept_span: &ScoredSpan| {
                if kept_span.label != span.label {
                    return false;
                }
                let inter_start = span.start.max(kept_span.start);
                let inter_end = span.end.min(kept_span.end);
                if inter_start >= inter_end {
                    return false;
                }
                let intersection = (inter_end - inter_start) as f32;
                let union =
                    (span.end - span.start + kept_span.end - kept_span.start) as f32 - intersection;
                intersection / union > IOU_THRESHOLD
            });

            if !dominated {
                kept.push(span);
            }
        }

        kept
    }

    fn extract_inner(&self, text: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        self.extract_inner_with_labels(text, &self.labels)
    }

    fn extract_inner_with_labels(
        &self,
        text: &str,
        labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        let text_words = Self::split_text_words(text);
        if text_words.is_empty() {
            return Ok(Vec::new());
        }

        let prompt_words = self.build_prompt_words_for_labels(labels);
        let prompt_word_count = prompt_words.len();

        let mut all_spans = Vec::new();

        let mut windows = Vec::new();
        let mut window_start = 0;
        while window_start < text_words.len() {
            let (encoding, window_end) =
                self.encode_window(&prompt_words, &text_words, window_start)?;
            windows.push(batching::EncodedWindow {
                input_ids: encoding.get_ids().to_vec(),
                word_ids: encoding.get_word_ids().to_vec(),
                window_start,
                window_end,
            });
            if window_end >= text_words.len() {
                break;
            }
            window_start = window_end.saturating_sub(1).max(window_start + 1);
        }

        for range in batching::pack_window_batches(&windows, self.batch_size, self.max_batch_tokens)
        {
            let batch = &windows[range];
            let hidden_rows = self.run_forward_batch(batch)?;
            for (window, hidden) in batch.iter().zip(hidden_rows) {
                self.decode_window(
                    text,
                    &text_words,
                    labels,
                    prompt_word_count,
                    window,
                    &hidden,
                    &mut all_spans,
                )?;
            }
        }

        let final_spans = self.apply_nms(all_spans);
        let mut candidates = final_spans
            .into_iter()
            .map(|span| EntityCandidate {
                entity_type: span.label,
                canonical_name: span.text,
                aliases: Vec::new(),
            })
            .collect::<Vec<_>>();

        candidates.sort_by(|left, right| left.canonical_name.cmp(&right.canonical_name));
        candidates.dedup_by(|left, right| {
            left.canonical_name == right.canonical_name && left.entity_type == right.entity_type
        });

        Ok(candidates)
    }

    fn build_prompt_words_for_labels(&self, labels: &[String]) -> Vec<String> {
        let ent_token = self
            .tokenizer
            .id_to_token(self.ent_token_id)
            .unwrap_or_else(|| "<<ENT>>".to_string());
        let mut prompt = Vec::with_capacity(labels.len() * 2 + 1);
        for label in labels {
            prompt.push(ent_token.clone());
            prompt.push(label.clone());
        }
        prompt.push(SEP_TOKEN.to_string());
        prompt
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
        let (_permit, queue_wait) = self
            .inference_gate
            .acquire()
            .await
            .map_err(|_| MemoryError::Storage("GLiNER inference gate closed".to_string()))?;
        self.logger.log(
            crate::service::log_event(
                "ner.gliner.queue.done",
                crate::service::log_args_with_duration(serde_json::json!({}), queue_wait),
                serde_json::json!({"available_permits": self.inference_gate.available_permits()}),
                None,
                None,
                None,
            ),
            crate::logging::LogLevel::Debug,
        );
        self.extract_inner(content)
    }

    async fn extract_candidates_with_labels(
        &self,
        content: &str,
        zero_shot_labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        let (_permit, queue_wait) = self
            .inference_gate
            .acquire()
            .await
            .map_err(|_| MemoryError::Storage("GLiNER inference gate closed".to_string()))?;
        self.logger.log(
            crate::service::log_event(
                "ner.gliner.queue.done",
                crate::service::log_args_with_duration(serde_json::json!({}), queue_wait),
                serde_json::json!({"available_permits": self.inference_gate.available_permits()}),
                None,
                None,
                None,
            ),
            crate::logging::LogLevel::Debug,
        );
        self.extract_inner_with_labels(content, zero_shot_labels)
    }
}

pub(crate) fn build_span_scoring_log_event(
    text_words: usize,
    span_count: usize,
    duration: std::time::Duration,
) -> HashMap<String, serde_json::Value> {
    crate::service::log_event(
        "ner.gliner.span_scores.done",
        crate::service::log_args_with_duration(
            serde_json::json!({"text_words": text_words}),
            duration,
        ),
        serde_json::json!({"span_count": span_count}),
        None,
        None,
        None,
    )
}
