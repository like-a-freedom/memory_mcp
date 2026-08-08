//! Bidirectional LFM2 GLiNER configuration parsing.
//!
//! Parses the upstream `gliner_config.json` of
//! `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` into a typed config consumed by
//! the native Candle backbone ([`super::model::Lfm2BiModel`]) and, in a later step,
//! by the span-decoding head.
//
// A module-level lint allowance covers the shared contract surface between
//! config parsing and span decoding.
#![allow(dead_code)]

use serde_json::Value;

use crate::service::MemoryError;

/// Tokenizer-resolved GLiNER separator token IDs for the
/// SauerkrautLM-LFM2.5-GLiNER checkpoint (`<<ENT>>` = 64402,
/// `<<SEP>>` = 64403). The checkpoint's `gliner_config.json` carries only the
/// token *strings* (`ent_token` / `sep_token`); the numeric IDs come from the
/// tokenizer, so these constants are the canonical fallback.
pub(crate) const DEFAULT_ENT_TOKEN_ID: u32 = 64_402;
pub(crate) const DEFAULT_SEP_TOKEN_ID: u32 = 64_403;

/// Head-side defaults applied when `gliner_config.json` omits a field.
const DEFAULT_MAX_WIDTH: usize = 12;
const DEFAULT_MAX_LEN: usize = 1024;
const DEFAULT_POST_FUSION_LAYERS: usize = 1;
const DEFAULT_RNN_LAYERS: usize = 1;

/// Kind of a single LFM2 decoder layer: depthwise short-conv or full attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayerKind {
    Conv,
    FullAttention,
}

impl LayerKind {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "conv" => Some(Self::Conv),
            "full_attention" => Some(Self::FullAttention),
            _ => None,
        }
    }
}

/// Native configuration of the bidirectional LFM2 GLiNER backbone.
#[derive(Debug, Clone)]
pub(crate) struct Lfm2BiConfig {
    pub(crate) vocab_size: usize,
    pub(crate) hidden_size: usize,
    /// Effective MLP width after `block_auto_adjust_ff_dim` rounding (4608 for
    /// the upstream checkpoint; `intermediate_size` 6656 -> 4608).
    pub(crate) intermediate_size: usize,
    pub(crate) num_hidden_layers: usize,
    pub(crate) num_attention_heads: usize,
    pub(crate) num_key_value_heads: usize,
    pub(crate) head_dim: usize,
    pub(crate) norm_eps: f64,
    pub(crate) rope_theta: f64,
    pub(crate) conv_l_cache: usize,
    pub(crate) layer_types: Vec<LayerKind>,
    pub(crate) max_width: usize,
    pub(crate) max_len: usize,
    pub(crate) ent_token_id: u32,
    pub(crate) sep_token_id: u32,
    pub(crate) class_token_index: u32,
    pub(crate) fuse_layers: bool,
    pub(crate) num_post_fusion_layers: usize,
    pub(crate) num_rnn_layers: usize,
    /// GLiNER span representation mode; this backend only supports `markerV1`.
    pub(crate) span_mode: String,
    /// Subtoken pooling for word embeddings; this backend only supports
    /// `first` (gather the first subtoken).
    pub(crate) subtoken_pooling: String,
}

impl Lfm2BiConfig {
    /// Parses an upstream `gliner_config.json` document.
    ///
    /// Backbone values are read from the nested `encoder_config` object when
    /// present (falling back to the top level); GLiNER head values (`max_width`,
    /// `max_len`, `class_token_index`, `fuse_layers`, ...) are read from the
    /// top level.
    pub(crate) fn from_gliner_config(json: &Value) -> Result<Self, MemoryError> {
        let encoder = json.get("encoder_config");
        let field = |key: &str| {
            encoder
                .and_then(|entry| entry.get(key))
                .or_else(|| json.get(key))
        };

        let get_usize = |key: &str| -> Result<usize, MemoryError> {
            field(key)
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .ok_or_else(|| {
                    MemoryError::ConfigInvalid(format!(
                        "gliner_config.json is missing numeric `{key}`"
                    ))
                })
        };
        let get_f64 = |key: &str| -> Result<f64, MemoryError> {
            field(key).and_then(Value::as_f64).ok_or_else(|| {
                MemoryError::ConfigInvalid(format!("gliner_config.json is missing numeric `{key}`"))
            })
        };
        let get_bool = |key: &str| -> Result<bool, MemoryError> {
            field(key).and_then(Value::as_bool).ok_or_else(|| {
                MemoryError::ConfigInvalid(format!("gliner_config.json is missing boolean `{key}`"))
            })
        };

        let vocab_size = get_usize("vocab_size")?;
        let hidden_size = get_usize("hidden_size")?;
        let num_hidden_layers = get_usize("num_hidden_layers")?;
        let num_attention_heads = get_usize("num_attention_heads")?;
        let num_key_value_heads = get_usize("num_key_value_heads")?;
        let norm_eps = get_f64("norm_eps")?;
        let conv_l_cache = get_usize("conv_L_cache")?;

        let raw_intermediate = get_usize("intermediate_size")?;
        let block_auto_adjust_ff_dim = get_bool("block_auto_adjust_ff_dim")?;
        let block_multiple_of = get_usize("block_multiple_of")?;
        let block_ffn_dim_multiplier = field("block_ffn_dim_multiplier").and_then(Value::as_f64);
        let intermediate_size = effective_intermediate_size(
            raw_intermediate,
            block_auto_adjust_ff_dim,
            block_multiple_of,
            block_ffn_dim_multiplier,
        );

        let rope_theta = field("rope_parameters")
            .and_then(|params| params.get("rope_theta"))
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                MemoryError::ConfigInvalid(
                    "gliner_config.json is missing `encoder_config.rope_parameters.rope_theta`"
                        .to_string(),
                )
            })?;

        let layer_types = field("layer_types")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry.as_str().and_then(LayerKind::parse).ok_or_else(|| {
                            MemoryError::ConfigInvalid(format!(
                                "gliner_config.json has an invalid `layer_types` entry: {entry}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .ok_or_else(|| {
                MemoryError::ConfigInvalid(
                    "gliner_config.json is missing `encoder_config.layer_types`".to_string(),
                )
            })?;

        if layer_types.len() != num_hidden_layers {
            return Err(MemoryError::ConfigInvalid(format!(
                "layer_types length {} does not match num_hidden_layers {num_hidden_layers}",
                layer_types.len()
            )));
        }
        if num_attention_heads == 0 || hidden_size % num_attention_heads != 0 {
            return Err(MemoryError::ConfigInvalid(format!(
                "hidden_size {hidden_size} must be a positive multiple of \
                 num_attention_heads {num_attention_heads}"
            )));
        }
        if num_key_value_heads == 0 || num_attention_heads % num_key_value_heads != 0 {
            return Err(MemoryError::ConfigInvalid(format!(
                "num_attention_heads {num_attention_heads} must be a positive multiple of \
                 num_key_value_heads {num_key_value_heads}"
            )));
        }
        let head_dim = hidden_size / num_attention_heads;

        let span_mode = json
            .get("span_mode")
            .and_then(Value::as_str)
            .unwrap_or("markerV1");
        if span_mode != "markerV1" {
            return Err(MemoryError::ConfigInvalid(format!(
                "gliner_config.json span_mode `{span_mode}` is unsupported; \
                 this backend only supports markerV1"
            )));
        }
        let subtoken_pooling = json
            .get("subtoken_pooling")
            .and_then(Value::as_str)
            .unwrap_or("first");
        if subtoken_pooling != "first" {
            return Err(MemoryError::ConfigInvalid(format!(
                "gliner_config.json subtoken_pooling `{subtoken_pooling}` is unsupported; \
                 this backend only supports first"
            )));
        }

        Ok(Self {
            vocab_size,
            hidden_size,
            intermediate_size,
            num_hidden_layers,
            num_attention_heads,
            num_key_value_heads,
            head_dim,
            norm_eps,
            rope_theta,
            conv_l_cache,
            layer_types,
            max_width: top_usize(json, "max_width").unwrap_or(DEFAULT_MAX_WIDTH),
            max_len: top_usize(json, "max_len").unwrap_or(DEFAULT_MAX_LEN),
            ent_token_id: top_u32(json, "ent_token_id").unwrap_or(DEFAULT_ENT_TOKEN_ID),
            sep_token_id: top_u32(json, "sep_token_id").unwrap_or(DEFAULT_SEP_TOKEN_ID),
            class_token_index: top_u32(json, "class_token_index").ok_or_else(|| {
                MemoryError::ConfigInvalid(
                    "gliner_config.json is missing numeric `class_token_index`".to_string(),
                )
            })?,
            fuse_layers: json
                .get("fuse_layers")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            num_post_fusion_layers: top_usize(json, "num_post_fusion_layers")
                .unwrap_or(DEFAULT_POST_FUSION_LAYERS),
            num_rnn_layers: top_usize(json, "num_rnn_layers").unwrap_or(DEFAULT_RNN_LAYERS),
            span_mode: span_mode.to_string(),
            subtoken_pooling: subtoken_pooling.to_string(),
        })
    }
}

/// Reads a top-level unsigned integer field.
fn top_usize(json: &Value, key: &str) -> Option<usize> {
    json.get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

/// Reads a top-level unsigned 32-bit field.
fn top_u32(json: &Value, key: &str) -> Option<u32> {
    json.get(key)
        .and_then(Value::as_u64)
        .map(|value| value as u32)
}

/// Mirrors the upstream `Lfm2MLP` intermediate-size adjustment:
/// `int(2 * raw / 3)`, then (only when a multiplier is configured) round up to
/// `block_multiple_of`.
fn effective_intermediate_size(
    raw: usize,
    auto_adjust: bool,
    block_multiple_of: usize,
    multiplier: Option<f64>,
) -> usize {
    let mut size = raw;
    if auto_adjust {
        size = (2 * size) / 3;
        if let Some(multiplier) = multiplier {
            size = (multiplier * size as f64) as usize;
            if block_multiple_of > 0 {
                size = block_multiple_of * size.div_ceil(block_multiple_of);
            }
        }
    }
    size
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{Device, Tensor};

    /// Literal copy of the upstream
    /// `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER/gliner_config.json`.
    const REAL_GLINER_CONFIG: &str = r#"{
      "bos_token_id": 1,
      "class_token_index": 64402,
      "dropout": 0.1,
      "embed_ent_token": true,
      "encoder_config": {
        "transformers_version": "5.8.1",
        "architectures": [
          "Lfm2BiModel"
        ],
        "output_hidden_states": false,
        "return_dict": true,
        "dtype": "float32",
        "chunk_size_feed_forward": 0,
        "is_encoder_decoder": false,
        "id2label": {
          "0": "LABEL_0",
          "1": "LABEL_1"
        },
        "label2id": {
          "LABEL_0": 0,
          "LABEL_1": 1
        },
        "problem_type": null,
        "vocab_size": 64404,
        "hidden_size": 1024,
        "intermediate_size": 6656,
        "num_hidden_layers": 16,
        "num_attention_heads": 16,
        "num_key_value_heads": 8,
        "max_position_embeddings": 128000,
        "initializer_range": 0.02,
        "norm_eps": 1e-05,
        "use_cache": true,
        "pad_token_id": 0,
        "bos_token_id": 1,
        "eos_token_id": 7,
        "tie_word_embeddings": true,
        "rope_parameters": {
          "rope_theta": 1000000.0,
          "rope_type": "default"
        },
        "conv_bias": false,
        "conv_L_cache": 3,
        "block_multiple_of": 256,
        "block_ffn_dim_multiplier": 1.0,
        "block_auto_adjust_ff_dim": true,
        "full_attn_idxs": null,
        "layer_types": [
          "conv",
          "conv",
          "full_attention",
          "conv",
          "conv",
          "full_attention",
          "conv",
          "conv",
          "full_attention",
          "conv",
          "full_attention",
          "conv",
          "full_attention",
          "conv",
          "full_attention",
          "conv"
        ],
        "_name_or_path": "/run/determined/workdir/mmontebovi/gliner_boost/models_lfm2_bi_mlm_v2/ckpt22500_backbone",
        "block_dim": 1024,
        "block_mlp_init_scale": 1.0,
        "block_norm_eps": 1e-05,
        "block_out_init_scale": 1.0,
        "block_use_swiglu": true,
        "block_use_xavier_init": true,
        "conv_dim": 1024,
        "conv_use_xavier_init": true,
        "model_type": "lfm2",
        "num_heads": 16,
        "use_pos_enc": true,
        "output_attentions": false
      },
      "ent_token": "<<ENT>>",
      "eos_token_id": 7,
      "fine_tune": true,
      "fuse_layers": true,
      "hidden_size": 1024,
      "labels_decoder": null,
      "labels_encoder": null,
      "max_len": 1024,
      "max_neg_type_ratio": 1,
      "max_types": 100,
      "max_width": 12,
      "model_name": "/run/determined/workdir/mmontebovi/gliner_boost/models_lfm2_bi_mlm_v2/ckpt22500_backbone",
      "model_type": null,
      "moe_aux_loss_coef": 0.0,
      "moe_bilinear": false,
      "moe_bilinear_gate_init_std": 0.02,
      "moe_bilinear_init_std": 0.01,
      "moe_bilinear_num_experts": 8,
      "moe_bilinear_rank": 32,
      "moe_bilinear_reg_coef": 0.001,
      "moe_drop_upcycle_p": 0.5,
      "moe_expert_dim": null,
      "moe_gate_init_std": 0.02,
      "moe_num_experts": null,
      "moe_num_topics": null,
      "moe_post_encoder": false,
      "moe_residual_scale": 0.1,
      "moe_shared_expert_dim": null,
      "moe_top_k": 2,
      "moe_topic_loss_coef": 0.1,
      "moe_topic_routed": false,
      "moe_use_shared_expert": false,
      "moe_weight_scale_exp": 0.3333333333333333,
      "moe_zloss_coef": 0.0,
      "name": "LFM2.5-350M +MLM-v2(ckpt22500) DENSE GLiNER \u2014 STAGE 2",
      "neg_spans_ratio": 1.0,
      "num_post_fusion_layers": 1,
      "num_rnn_layers": 1,
      "pad_token_id": 0,
      "post_fusion_schema": null,
      "represent_spans": false,
      "sep_token": "<<SEP>>",
      "span_loss_coef": 1.0,
      "span_mode": "markerV1",
      "subtoken_pooling": "first",
      "token_loss_coef": 1.0,
      "transformers_version": "5.8.1",
      "use_cache": false,
      "vocab_size": 64404,
      "words_splitter_type": "whitespace"
    }"#;

    fn parsed() -> Lfm2BiConfig {
        let json: Value = serde_json::from_str(REAL_GLINER_CONFIG).expect("valid JSON literal");
        Lfm2BiConfig::from_gliner_config(&json).expect("config parses")
    }

    #[test]
    fn parses_real_gliner_config_values() {
        let config = parsed();
        assert_eq!(config.vocab_size, 64_404);
        assert_eq!(config.hidden_size, 1_024);
        assert_eq!(config.intermediate_size, 4_608);
        assert_eq!(config.num_hidden_layers, 16);
        assert_eq!(config.num_attention_heads, 16);
        assert_eq!(config.num_key_value_heads, 8);
        assert_eq!(config.head_dim, 64);
        assert_eq!(config.norm_eps, 1e-5);
        assert_eq!(config.rope_theta, 1e6);
        assert_eq!(config.conv_l_cache, 3);
        assert_eq!(config.max_width, 12);
        assert_eq!(config.max_len, 1024);
        assert_eq!(config.ent_token_id, 64_402);
        assert_eq!(config.sep_token_id, 64_403);
        assert_eq!(config.class_token_index, 64_402);
        assert!(config.fuse_layers);
        assert_eq!(config.num_post_fusion_layers, 1);
        assert_eq!(config.num_rnn_layers, 1);
        assert_eq!(config.span_mode, "markerV1");
        assert_eq!(config.subtoken_pooling, "first");
    }

    #[test]
    fn unsupported_span_mode_is_rejected() {
        let json: Value = serde_json::from_str(REAL_GLINER_CONFIG).expect("valid JSON literal");
        let mut object = json.as_object().expect("config object").clone();
        object.insert(
            "span_mode".to_string(),
            Value::String("markerV2".to_string()),
        );
        let error = Lfm2BiConfig::from_gliner_config(&Value::Object(object))
            .expect_err("unsupported span_mode must fail");
        assert!(
            error.to_string().contains("markerV1"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn unsupported_subtoken_pooling_is_rejected() {
        let json: Value = serde_json::from_str(REAL_GLINER_CONFIG).expect("valid JSON literal");
        let mut object = json.as_object().expect("config object").clone();
        object.insert(
            "subtoken_pooling".to_string(),
            Value::String("mean".to_string()),
        );
        let error = Lfm2BiConfig::from_gliner_config(&Value::Object(object))
            .expect_err("unsupported subtoken_pooling must fail");
        assert!(
            error.to_string().contains("first"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn layer_types_follow_conv_attention_schedule() {
        let config = parsed();
        let expected = [
            LayerKind::Conv,
            LayerKind::Conv,
            LayerKind::FullAttention,
            LayerKind::Conv,
            LayerKind::Conv,
            LayerKind::FullAttention,
            LayerKind::Conv,
            LayerKind::Conv,
            LayerKind::FullAttention,
            LayerKind::Conv,
            LayerKind::FullAttention,
            LayerKind::Conv,
            LayerKind::FullAttention,
            LayerKind::Conv,
            LayerKind::FullAttention,
            LayerKind::Conv,
        ];
        assert_eq!(config.layer_types, expected);

        let attention_at: Vec<usize> = (0..config.layer_types.len())
            .filter(|&index| config.layer_types[index] == LayerKind::FullAttention)
            .collect();
        assert_eq!(attention_at, vec![2, 5, 8, 10, 12, 14]);
        let conv_count = config
            .layer_types
            .iter()
            .filter(|&&kind| kind == LayerKind::Conv)
            .count();
        assert_eq!(conv_count, 10);
    }

    #[test]
    fn effective_intermediate_size_matches_upstream_rounding() {
        // 6656 -> int(2*6656/3) = 4437 -> round up to 256 -> 4608.
        assert_eq!(
            effective_intermediate_size(6656, true, 256, Some(1.0)),
            4608
        );
        // No multiplier configured: truncation only, no multiple-of rounding.
        assert_eq!(effective_intermediate_size(6656, true, 256, None), 4437);
        // No auto-adjust: the raw value is used verbatim.
        assert_eq!(
            effective_intermediate_size(6656, false, 256, Some(1.0)),
            6656
        );
    }

    #[test]
    fn symmetric_center_padded_conv_preserves_length_and_centering() {
        let device = Device::Cpu;
        let input = Tensor::from_vec(vec![1f32, 2., 3., 4., 5.], (1, 1, 5), &device).unwrap();
        // Center tap: out[i] == in[i] and the output length matches the input.
        let center = Tensor::from_vec(vec![0f32, 1., 0.], (1, 1, 3), &device).unwrap();
        let out = input.conv1d(&center, 1, 1, 1, 1).unwrap();
        assert_eq!(out.dims(), &[1, 1, 5]);
        let values = out.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(values, vec![1., 2., 3., 4., 5.]);
    }

    #[test]
    fn symmetric_conv_position_zero_reads_the_future() {
        let device = Device::Cpu;
        let input = Tensor::from_vec(vec![1f32, 2., 3., 4., 5.], (1, 1, 5), &device).unwrap();
        // Candle's conv1d indexes `w[k]` against input[l - padding + k], so a
        // kernel tap on the *last* position reads the following token. With
        // symmetric (center) padding the output at position 0 therefore depends
        // on input position 1 — impossible under the original causal
        // (left-only) padding where position 0 can only ever see itself.
        let right_tap = Tensor::from_vec(vec![0f32, 0., 1.], (1, 1, 3), &device).unwrap();
        let out = input.conv1d(&right_tap, 1, 1, 1, 1).unwrap();
        let values = out.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(values, vec![2., 3., 4., 5., 0.]);
    }

    #[test]
    fn candle_conv1d_matches_pytorch_cross_correlation() {
        // Pin the conv orientation independently of the checkpoint: candle's
        // conv1d must equal PyTorch's nn.Conv1d (cross-correlation),
        // `out[l] = sum_k input[l + k - padding] * w[k]`, for an asymmetric
        // kernel. Hand-computed over input [1..5], kernel [1,2,3], padding 1:
        //   out[0] = in[-1]*1 + in[0]*2 + in[1]*3 = 0 + 2 + 6  = 8
        //   out[1] = in[0]*1  + in[1]*2 + in[2]*3 = 1 + 4 + 9  = 14
        //   out[2] = in[1]*1  + in[2]*2 + in[3]*3 = 2 + 6 + 12 = 20
        //   out[3] = in[2]*1  + in[3]*2 + in[4]*3 = 3 + 8 + 15 = 26
        //   out[4] = in[3]*1  + in[4]*2 + in[5]*3 = 4 + 10 + 0 = 14
        let device = Device::Cpu;
        let input = Tensor::from_vec(vec![1f32, 2., 3., 4., 5.], (1, 1, 5), &device).unwrap();
        let kernel = Tensor::from_vec(vec![1f32, 2., 3.], (1, 1, 3), &device).unwrap();
        let out = input.conv1d(&kernel, 1, 1, 1, 1).unwrap();
        let values = out.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(values, vec![8., 14., 20., 26., 14.]);
        // A flipped kernel would produce [4, 10, 16, 22, 22] — assert that is
        // NOT the output so a regression to the flip is caught.
        let flipped = kernel.flip(&[2]).unwrap();
        let flipped_out = input.conv1d(&flipped, 1, 1, 1, 1).unwrap();
        let flipped_values = flipped_out.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(flipped_values, vec![4., 10., 16., 22., 22.]);
    }

    #[test]
    fn depthwise_conv_groups_keep_channels_independent() {
        let device = Device::Cpu;
        let input = Tensor::from_vec(
            vec![1f32, 2., 3., 4., 5., 10., 20., 30., 40., 50.],
            (1, 2, 5),
            &device,
        )
        .unwrap();
        // Channel 0 gets a center tap (identity), channel 1 a leading tap
        // (candle orientation: out[l] = in[l - 1]); each channel must be
        // convolved with only its own kernel (groups == in_channels).
        let kernel = Tensor::from_vec(vec![0f32, 1., 0., 1., 0., 0.], (2, 1, 3), &device).unwrap();
        let out = input.conv1d(&kernel, 1, 1, 1, 2).unwrap();
        let values = out.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(values, vec![1., 2., 3., 4., 5., 0., 10., 20., 30., 40.]);
    }
}
